// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Notes reported by run wrapper scripts.

use crate::{
    errors::{ChildStartError, RunWrapperReportError},
    reporter::events::RunWrapperReport,
};
use camino::Utf8Path;
use camino_tempfile::Utf8TempDir;
use std::{io, sync::Arc};
use tokio::io::AsyncReadExt;

pub(super) const RUN_WRAPPER_REPORT_ENV: &str = "NEXTEST_RUN_WRAPPER_REPORT";

// Allow both maximum-length fields even when every character is JSON-escaped.
const MAX_REPORT_SIZE: u64 = 4096;

pub(super) fn new_report_dir() -> Result<Utf8TempDir, ChildStartError> {
    camino_tempfile::Builder::new()
        .prefix("nextest-run-wrapper-report")
        .tempdir()
        .map_err(|error| ChildStartError::RunWrapperReportTempDir(Arc::new(error)))
}

pub(super) async fn read_report(
    path: &Utf8Path,
) -> Result<Option<RunWrapperReport>, RunWrapperReportError> {
    let metadata = match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RunWrapperReportError::Metadata(Arc::new(error))),
    };
    // tokio::fs can hang when opening a FIFO without a writer.
    if !metadata.is_file() {
        return Err(RunWrapperReportError::NotRegularFile);
    }

    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    // A FIFO substituted after the metadata check must not block the open.
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = match options.open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RunWrapperReportError::Open(Arc::new(error))),
    };
    // Check the opened file as well, since the path may have changed.
    if !file
        .metadata()
        .await
        .map_err(|error| RunWrapperReportError::Metadata(Arc::new(error)))?
        .is_file()
    {
        return Err(RunWrapperReportError::NotRegularFile);
    }

    let mut contents = Vec::new();
    file.take(MAX_REPORT_SIZE + 1)
        .read_to_end(&mut contents)
        .await
        .map_err(|error| RunWrapperReportError::Read(Arc::new(error)))?;
    if contents.len() as u64 > MAX_REPORT_SIZE {
        return Err(RunWrapperReportError::TooLarge {
            max_size: MAX_REPORT_SIZE,
        });
    }

    let report: RunWrapperReport = serde_json::from_slice(&contents)
        .map_err(|error| RunWrapperReportError::Parse(Arc::new(error)))?;
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reporter::events::{RunWrapperCategory, RunWrapperLabel};
    use std::fs;

    fn report_dir(contents: &[u8]) -> Utf8TempDir {
        let dir = new_report_dir().unwrap();
        fs::write(dir.path().join("report.json"), contents).unwrap();
        dir
    }

    #[tokio::test]
    async fn valid_report_is_loaded() {
        let dir = report_dir(
            br#"{"label":"not cached: external read (outside workspace) seen","category":"io"}"#,
        );
        let report = read_report(&dir.path().join("report.json"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            report.label.as_str(),
            "not cached: external read (outside workspace) seen"
        );
        assert_eq!(
            report.category.as_ref().map(|category| category.as_str()),
            Some("io")
        );
    }

    #[tokio::test]
    async fn category_is_optional() {
        let dir = report_dir(br#"{"label":"cached"}"#);
        assert_eq!(
            read_report(&dir.path().join("report.json"))
                .await
                .unwrap()
                .unwrap()
                .category,
            None
        );
    }

    #[tokio::test]
    async fn absent_report_is_not_an_error() {
        let dir = new_report_dir().unwrap();
        assert_eq!(
            read_report(&dir.path().join("report.json")).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn directory_reports_are_rejected() {
        let dir = new_report_dir().unwrap();
        let path = dir.path().join("report.json");
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            read_report(&path).await,
            Err(RunWrapperReportError::NotRegularFile)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fifo_reports_are_rejected_without_a_writer() {
        use std::{ffi::CString, os::unix::fs::symlink};

        let dir = new_report_dir().unwrap();
        let path = dir.path().join("report.json");
        let c_path = CString::new(path.as_str()).unwrap();
        // SAFETY: c_path is a valid null-terminated string.
        let result = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(result, 0, "mkfifo failed: {}", io::Error::last_os_error());
        let link = dir.path().join("report-link.json");
        symlink(&path, &link).unwrap();
        for path in [&path, &link] {
            assert!(matches!(
                read_report(path).await,
                Err(RunWrapperReportError::NotRegularFile)
            ));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_to_regular_reports_are_loaded() {
        use std::os::unix::fs::symlink;

        let dir = report_dir(br#"{"label":"cached"}"#);
        let link = dir.path().join("report-link.json");
        symlink(dir.path().join("report.json"), &link).unwrap();
        assert_eq!(
            read_report(&link).await.unwrap().unwrap().label.as_str(),
            "cached"
        );
    }

    #[tokio::test]
    async fn invalid_reports_are_errors() {
        for contents in [
            br#"not json"#.as_slice(),
            br#"{"label":""}"#,
            br#"{"label":" leading space"}"#,
            br#"{"label":"trailing space "}"#,
            br#"{"label":"bad\nlabel"}"#,
            br#"{"label":"ends with punctuation)"}"#,
            br#"{"label":"caf\u00e9"}"#,
            br#"{"label":"cached","category":""}"#,
            br#"{"label":"cached","category":" leading space"}"#,
            br#"{"label":"cached","category":"bad\ncategory"}"#,
        ] {
            let dir = report_dir(contents);
            assert!(read_report(&dir.path().join("report.json")).await.is_err());
        }
    }

    #[tokio::test]
    async fn length_limits_are_enforced() {
        let long_label = "x".repeat(RunWrapperLabel::MAX_LEN + 1);
        let long_category = "x".repeat(RunWrapperCategory::MAX_LEN + 1);
        for contents in [
            format!(r#"{{"label":"{long_label}"}}"#),
            format!(r#"{{"label":"cached","category":"{long_category}"}}"#),
        ] {
            let dir = report_dir(contents.as_bytes());
            assert!(read_report(&dir.path().join("report.json")).await.is_err());
        }

        let max_label = "x".repeat(RunWrapperLabel::MAX_LEN);
        let dir = report_dir(format!(r#"{{"label":"{max_label}"}}"#).as_bytes());
        assert_eq!(
            read_report(&dir.path().join("report.json"))
                .await
                .unwrap()
                .unwrap()
                .label
                .as_str(),
            max_label
        );
    }

    #[tokio::test]
    async fn escaped_maximum_length_fields_are_accepted() {
        let contents = format!(
            r#"{{"label":"{}","category":"{}"}}"#,
            "\\u0061".repeat(RunWrapperLabel::MAX_LEN),
            "\\u0062".repeat(RunWrapperCategory::MAX_LEN)
        );
        let dir = report_dir(contents.as_bytes());
        let report = read_report(&dir.path().join("report.json"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(report.label.as_str(), "a".repeat(RunWrapperLabel::MAX_LEN));
        assert_eq!(
            report.category.unwrap().as_str(),
            "b".repeat(RunWrapperCategory::MAX_LEN)
        );
    }

    #[test]
    fn report_directories_are_isolated_and_removed() {
        let first = report_dir(b"first");
        let second = report_dir(b"second");
        let first_path = first.path().to_owned();
        assert_ne!(first.path(), second.path());
        drop(first);
        assert!(!first_path.exists());
        assert_eq!(
            fs::read(second.path().join("report.json")).unwrap(),
            b"second"
        );
    }

    #[tokio::test]
    async fn oversized_reports_are_errors() {
        let dir = report_dir(&vec![b'x'; MAX_REPORT_SIZE as usize + 1]);
        assert!(read_report(&dir.path().join("report.json")).await.is_err());
    }
}
