// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Notes reported by run wrapper scripts.

use crate::{errors::ChildStartError, reporter::events::RunWrapperReport};
use camino_tempfile::Utf8TempPath;
use std::{
    fs::{self, File},
    io::{self, Read},
    sync::Arc,
};
use thiserror::Error;
use tracing::warn;

pub(super) const RUN_WRAPPER_REPORT_ENV: &str = "NEXTEST_RUN_WRAPPER_REPORT";

const MAX_REPORT_SIZE: u64 = 1024;
const MAX_LABEL_LEN: usize = 256;
const MAX_GROUP_LEN: usize = 64;

#[derive(Debug, Error)]
enum RunWrapperReportError {
    #[error("failed to open the report: {0}")]
    Open(#[source] io::Error),
    #[error("failed to read the report: {0}")]
    Read(#[source] io::Error),
    #[error("the report exceeds {MAX_REPORT_SIZE} bytes")]
    TooLarge,
    #[error("failed to parse the report: {0}")]
    Parse(#[source] serde_json::Error),
    #[error(
        "the label must contain 1 to {MAX_LABEL_LEN} printable ASCII characters, and must start and end with a letter or digit"
    )]
    InvalidLabel,
    #[error(
        "the group must contain 1 to {MAX_GROUP_LEN} printable ASCII characters, and must start and end with a letter or digit"
    )]
    InvalidGroup,
}

pub(super) fn new_report_path() -> Result<Utf8TempPath, ChildStartError> {
    let path = camino_tempfile::Builder::new()
        .prefix("nextest-run-wrapper-report")
        .tempfile()
        .map_err(|error| ChildStartError::TempPath(Arc::new(error)))?
        .into_temp_path();

    // The absence of a file means that the wrapper ran the test normally.
    fs::remove_file(&path).map_err(|error| ChildStartError::TempPath(Arc::new(error)))?;
    Ok(path)
}

pub(super) fn read_report(path: Option<&Utf8TempPath>) -> Option<RunWrapperReport> {
    let path = path?;

    match try_read_report(path) {
        Ok(report) => report,
        Err(error) => {
            warn!(%error, path = path.as_str(), "failed to read run wrapper report");
            None
        }
    }
}

fn try_read_report(path: &Utf8TempPath) -> Result<Option<RunWrapperReport>, RunWrapperReportError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RunWrapperReportError::Open(error)),
    };

    let mut contents = Vec::new();
    file.take(MAX_REPORT_SIZE + 1)
        .read_to_end(&mut contents)
        .map_err(RunWrapperReportError::Read)?;
    if contents.len() as u64 > MAX_REPORT_SIZE {
        return Err(RunWrapperReportError::TooLarge);
    }

    let report: RunWrapperReport =
        serde_json::from_slice(&contents).map_err(RunWrapperReportError::Parse)?;
    if !valid_text(&report.label, MAX_LABEL_LEN) {
        return Err(RunWrapperReportError::InvalidLabel);
    }
    if report
        .group
        .as_deref()
        .is_some_and(|group| !valid_text(group, MAX_GROUP_LEN))
    {
        return Err(RunWrapperReportError::InvalidGroup);
    }

    Ok(Some(report))
}

fn valid_text(value: &str, max_len: usize) -> bool {
    // Reports come from arbitrary wrapper scripts: restrict them to printable
    // ASCII with alphanumeric first and last characters.
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte == b' ' || byte.is_ascii_graphic())
        && value
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn report_path(contents: &[u8]) -> Utf8TempPath {
        let mut file = camino_tempfile::NamedUtf8TempFile::new().unwrap();
        file.write_all(contents).unwrap();
        file.into_temp_path()
    }

    #[test]
    fn valid_report_is_loaded() {
        let path = report_path(
            br#"{"label":"not cached: external read (outside workspace) seen","group":"io"}"#,
        );
        let report = read_report(Some(&path)).unwrap();
        assert_eq!(
            report.label,
            "not cached: external read (outside workspace) seen"
        );
        assert_eq!(report.group.as_deref(), Some("io"));
    }

    #[test]
    fn group_is_optional() {
        let path = report_path(br#"{"label":"cached"}"#);
        assert_eq!(read_report(Some(&path)).unwrap().group, None);
    }

    #[test]
    fn absent_report_is_not_an_error() {
        let path = new_report_path().unwrap();
        assert_eq!(read_report(Some(&path)), None);
    }

    #[test]
    fn invalid_reports_are_ignored() {
        for contents in [
            br#"not json"#.as_slice(),
            br#"{"label":""}"#,
            br#"{"label":" leading space"}"#,
            br#"{"label":"trailing space "}"#,
            br#"{"label":"bad\nlabel"}"#,
            br#"{"label":"ends with punctuation)"}"#,
            br#"{"label":"caf\u00e9"}"#,
            br#"{"label":"cached","group":""}"#,
            br#"{"label":"cached","group":" leading space"}"#,
            br#"{"label":"cached","group":"bad\ngroup"}"#,
        ] {
            let path = report_path(contents);
            assert_eq!(read_report(Some(&path)), None);
        }
    }

    #[test]
    fn length_limits_are_enforced() {
        let long_label = "x".repeat(MAX_LABEL_LEN + 1);
        let long_group = "x".repeat(MAX_GROUP_LEN + 1);
        for contents in [
            format!(r#"{{"label":"{long_label}"}}"#),
            format!(r#"{{"label":"cached","group":"{long_group}"}}"#),
        ] {
            let path = report_path(contents.as_bytes());
            assert_eq!(read_report(Some(&path)), None);
        }

        let max_label = "x".repeat(MAX_LABEL_LEN);
        let path = report_path(format!(r#"{{"label":"{max_label}"}}"#).as_bytes());
        assert_eq!(read_report(Some(&path)).unwrap().label, max_label);
    }

    #[test]
    fn oversized_reports_are_ignored() {
        let path = report_path(&vec![b'x'; MAX_REPORT_SIZE as usize + 1]);
        assert_eq!(read_report(Some(&path)), None);
    }
}
