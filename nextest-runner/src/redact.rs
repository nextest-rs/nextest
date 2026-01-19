// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Redact data that varies by system and OS to produce a stable output.
//!
//! Used for snapshot testing.

use crate::{
    helpers::{
        FormattedDuration, FormattedRelativeDuration, convert_rel_path_to_forward_slash,
        u64_decimal_char_width,
    },
    list::RustBuildMeta,
};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, TimeZone};
use regex::Regex;
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, LazyLock},
    time::Duration,
};

static CRATE_NAME_HASH_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-zA-Z0-9_-]+)-[a-f0-9]{16}$").unwrap());
static TARGET_DIR_REDACTION: &str = "<target-dir>";
static FILE_COUNT_REDACTION: &str = "<file-count>";
static DURATION_REDACTION: &str = "<duration>";

// Fixed-width placeholders for store list alignment.
// These match the original field widths to preserve column alignment.

/// 19 chars, matches `%Y-%m-%d %H:%M:%S` format.
static TIMESTAMP_REDACTION: &str = "XXXX-XX-XX XX:XX:XX";
/// 6 chars for numeric portion (e.g. "   123" for KB display).
static SIZE_REDACTION: &str = "<size>";
/// Placeholder for redacted version strings.
static VERSION_REDACTION: &str = "<version>";
/// Placeholder for redacted relative durations (e.g. "30s ago").
static RELATIVE_DURATION_REDACTION: &str = "<ago>";

/// A helper for redacting data that varies by environment.
///
/// This isn't meant to be perfect, and not everything can be redacted yet -- the set of supported
/// redactions will grow over time.
#[derive(Clone, Debug)]
pub struct Redactor {
    kind: Arc<RedactorKind>,
}

impl Redactor {
    /// Creates a new no-op redactor.
    pub fn noop() -> Self {
        Self::new_with_kind(RedactorKind::Noop)
    }

    fn new_with_kind(kind: RedactorKind) -> Self {
        Self {
            kind: Arc::new(kind),
        }
    }

    /// Creates a new redactor builder that operates on the given build metadata.
    ///
    /// This should only be called if redaction is actually needed.
    pub fn build_active<State>(build_meta: &RustBuildMeta<State>) -> RedactorBuilder {
        let mut redactions = Vec::new();

        let linked_path_redactions =
            build_linked_path_redactions(build_meta.linked_paths.keys().map(|p| p.as_ref()));

        // For all linked paths, push both absolute and relative redactions.
        for (source, replacement) in linked_path_redactions {
            redactions.push(Redaction::Path {
                path: build_meta.target_directory.join(&source),
                replacement: format!("{TARGET_DIR_REDACTION}/{replacement}"),
            });
            redactions.push(Redaction::Path {
                path: source,
                replacement,
            });
        }

        // Also add a redaction for the target directory. This goes after the linked paths, so that
        // absolute linked paths are redacted first.
        redactions.push(Redaction::Path {
            path: build_meta.target_directory.clone(),
            replacement: "<target-dir>".to_string(),
        });

        RedactorBuilder { redactions }
    }

    /// Redacts a path.
    pub fn redact_path<'a>(&self, orig: &'a Utf8Path) -> RedactorOutput<&'a Utf8Path> {
        for redaction in self.kind.iter_redactions() {
            match redaction {
                Redaction::Path { path, replacement } => {
                    if let Ok(suffix) = orig.strip_prefix(path) {
                        if suffix.as_str().is_empty() {
                            return RedactorOutput::Redacted(replacement.clone());
                        } else {
                            // Always use "/" as the separator, even on Windows, to ensure stable
                            // output across OSes.
                            let path = Utf8PathBuf::from(format!("{replacement}/{suffix}"));
                            return RedactorOutput::Redacted(
                                convert_rel_path_to_forward_slash(&path).into(),
                            );
                        }
                    }
                }
            }
        }

        RedactorOutput::Unredacted(orig)
    }

    /// Redacts a file count.
    pub fn redact_file_count(&self, orig: usize) -> RedactorOutput<usize> {
        if self.kind.is_active() {
            RedactorOutput::Redacted(FILE_COUNT_REDACTION.to_string())
        } else {
            RedactorOutput::Unredacted(orig)
        }
    }

    /// Redacts a duration.
    pub(crate) fn redact_duration(&self, orig: Duration) -> RedactorOutput<FormattedDuration> {
        if self.kind.is_active() {
            RedactorOutput::Redacted(DURATION_REDACTION.to_string())
        } else {
            RedactorOutput::Unredacted(FormattedDuration(orig))
        }
    }

    /// Returns true if this redactor is active (will redact values).
    pub fn is_active(&self) -> bool {
        self.kind.is_active()
    }

    /// Creates a new redactor for snapshot testing, without any path redactions.
    ///
    /// This is useful when you need redaction of timestamps, durations, and
    /// sizes, but don't have a `RustBuildMeta` to build path redactions from.
    pub fn for_snapshot_testing() -> Self {
        Self::new_with_kind(RedactorKind::Active {
            redactions: Vec::new(),
        })
    }

    /// Redacts a timestamp for display, producing a fixed-width placeholder.
    ///
    /// The placeholder `XXXX-XX-XX XX:XX:XX` is 19 characters, matching the
    /// width of the `%Y-%m-%d %H:%M:%S` format.
    pub fn redact_timestamp<Tz>(&self, orig: &DateTime<Tz>) -> RedactorOutput<DisplayTimestamp<Tz>>
    where
        Tz: TimeZone + Clone,
        Tz::Offset: fmt::Display,
    {
        if self.kind.is_active() {
            RedactorOutput::Redacted(TIMESTAMP_REDACTION.to_string())
        } else {
            RedactorOutput::Unredacted(DisplayTimestamp(orig.clone()))
        }
    }

    /// Redacts a size (in bytes) for display as a human-readable string.
    ///
    /// When redacting, produces `<size>` as a placeholder.
    pub fn redact_size(&self, orig: u64) -> RedactorOutput<SizeDisplay> {
        if self.kind.is_active() {
            RedactorOutput::Redacted(SIZE_REDACTION.to_string())
        } else {
            RedactorOutput::Unredacted(SizeDisplay(orig))
        }
    }

    /// Redacts a version for display.
    ///
    /// When redacting, produces `<version>` as a placeholder.
    pub fn redact_version(&self, orig: &semver::Version) -> String {
        if self.kind.is_active() {
            VERSION_REDACTION.to_string()
        } else {
            orig.to_string()
        }
    }

    /// Redacts a store duration for display, producing a fixed-width placeholder.
    ///
    /// The placeholder `<duration>` is 10 characters, matching the width of the
    /// `{:>9.3}s` format used for durations.
    pub fn redact_store_duration(&self, orig: Option<f64>) -> RedactorOutput<StoreDurationDisplay> {
        if self.kind.is_active() {
            RedactorOutput::Redacted(format!("{:>10}", DURATION_REDACTION))
        } else {
            RedactorOutput::Unredacted(StoreDurationDisplay(orig))
        }
    }

    /// Redacts a timestamp with timezone for detailed display.
    ///
    /// Produces `XXXX-XX-XX XX:XX:XX` when active, otherwise formats as
    /// `%Y-%m-%d %H:%M:%S %:z`.
    pub fn redact_detailed_timestamp<Tz>(&self, orig: &DateTime<Tz>) -> String
    where
        Tz: TimeZone,
        Tz::Offset: fmt::Display,
    {
        if self.kind.is_active() {
            TIMESTAMP_REDACTION.to_string()
        } else {
            orig.format("%Y-%m-%d %H:%M:%S %:z").to_string()
        }
    }

    /// Redacts a duration in seconds for detailed display.
    ///
    /// Produces `<duration>` when active, otherwise formats as `{:.3}s`.
    pub fn redact_detailed_duration(&self, orig: Option<f64>) -> String {
        if self.kind.is_active() {
            DURATION_REDACTION.to_string()
        } else {
            match orig {
                Some(secs) => format!("{:.3}s", secs),
                None => "-".to_string(),
            }
        }
    }

    /// Redacts a relative duration for display (e.g. "30s ago").
    ///
    /// Produces `<ago>` when active, otherwise formats the duration.
    pub(crate) fn redact_relative_duration(
        &self,
        orig: Duration,
    ) -> RedactorOutput<FormattedRelativeDuration> {
        if self.kind.is_active() {
            RedactorOutput::Redacted(RELATIVE_DURATION_REDACTION.to_string())
        } else {
            RedactorOutput::Unredacted(FormattedRelativeDuration(orig))
        }
    }

    /// Redacts CLI args for display.
    ///
    /// - The first arg (the exe) is replaced with `[EXE]`
    /// - Absolute paths in other args are replaced with `[PATH]`
    pub fn redact_cli_args(&self, args: &[String]) -> String {
        if !self.kind.is_active() {
            return shell_words::join(args);
        }

        let redacted: Vec<_> = args
            .iter()
            .enumerate()
            .map(|(i, arg)| {
                if i == 0 {
                    // First arg is always the exe.
                    "[EXE]".to_string()
                } else if is_absolute_path(arg) {
                    "[PATH]".to_string()
                } else {
                    arg.clone()
                }
            })
            .collect();
        shell_words::join(&redacted)
    }

    /// Redacts env vars for display.
    ///
    /// Formats as `K=V` pairs.
    pub fn redact_env_vars(&self, env_vars: &BTreeMap<String, String>) -> String {
        let pairs: Vec<_> = env_vars
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    shell_words::quote(k),
                    shell_words::quote(self.redact_env_value(v)),
                )
            })
            .collect();
        pairs.join(" ")
    }

    /// Redacts an env var value for display.
    ///
    /// Absolute paths are replaced with `[PATH]`.
    pub fn redact_env_value<'a>(&self, value: &'a str) -> &'a str {
        if self.kind.is_active() && is_absolute_path(value) {
            "[PATH]"
        } else {
            value
        }
    }
}

/// Returns true if the string looks like an absolute path.
fn is_absolute_path(s: &str) -> bool {
    s.starts_with('/') || (s.len() >= 3 && s.chars().nth(1) == Some(':'))
}

/// Wrapper for timestamps that formats with `%Y-%m-%d %H:%M:%S`.
#[derive(Clone, Debug)]
pub struct DisplayTimestamp<Tz: TimeZone>(pub DateTime<Tz>);

impl<Tz: TimeZone> fmt::Display for DisplayTimestamp<Tz>
where
    Tz::Offset: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.format("%Y-%m-%d %H:%M:%S"))
    }
}

/// Wrapper for store durations that formats as `{:>9.3}s` or `{:>10}` for "-".
#[derive(Clone, Debug)]
pub struct StoreDurationDisplay(pub Option<f64>);

impl fmt::Display for StoreDurationDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(secs) => write!(f, "{secs:>9.3}s"),
            None => write!(f, "{:>10}", "-"),
        }
    }
}

/// Wrapper for sizes that formats bytes as a human-readable string (KB or MB).
#[derive(Clone, Copy, Debug)]
pub struct SizeDisplay(pub u64);

impl SizeDisplay {
    /// Returns the display width of this size when formatted.
    ///
    /// This is useful for alignment calculations.
    pub fn display_width(self) -> usize {
        let bytes = self.0;
        if bytes >= 1024 * 1024 {
            // Format: "{:.1} MB" - integer part + "." + 1 decimal + " MB".
            let mb_int = bytes / (1024 * 1024);
            u64_decimal_char_width(mb_int) + 2 + 3
        } else {
            // Format: "{} KB" - integer + " KB".
            let kb = bytes / 1024;
            u64_decimal_char_width(kb) + 3
        }
    }
}

impl fmt::Display for SizeDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0;
        // Remove 3 from the width since we're adding " MB" or " KB" at the end.
        match (bytes >= 1024 * 1024, f.width().map(|w| w.saturating_sub(3))) {
            (true, Some(width)) => {
                write!(f, "{:>width$.1} MB", bytes as f64 / (1024.0 * 1024.0))
            }
            (true, None) => {
                write!(f, "{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
            }
            (false, Some(width)) => {
                write!(f, "{:>width$} KB", bytes / 1024)
            }
            (false, None) => {
                write!(f, "{} KB", bytes / 1024)
            }
        }
    }
}

/// A builder for [`Redactor`] instances.
///
/// Created with [`Redactor::build_active`].
#[derive(Debug)]
pub struct RedactorBuilder {
    redactions: Vec<Redaction>,
}

impl RedactorBuilder {
    /// Adds a new path redaction.
    pub fn with_path(mut self, path: Utf8PathBuf, replacement: String) -> Self {
        self.redactions.push(Redaction::Path { path, replacement });
        self
    }

    /// Builds the redactor.
    pub fn build(self) -> Redactor {
        Redactor::new_with_kind(RedactorKind::Active {
            redactions: self.redactions,
        })
    }
}

/// The output of a [`Redactor`] operation.
#[derive(Debug)]
pub enum RedactorOutput<T> {
    /// The value was not redacted.
    Unredacted(T),

    /// The value was redacted.
    Redacted(String),
}

impl<T: fmt::Display> fmt::Display for RedactorOutput<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RedactorOutput::Unredacted(value) => value.fmt(f),
            RedactorOutput::Redacted(replacement) => replacement.fmt(f),
        }
    }
}

#[derive(Debug)]
enum RedactorKind {
    Noop,
    Active {
        /// The list of redactions to apply.
        redactions: Vec<Redaction>,
    },
}

impl RedactorKind {
    fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    fn iter_redactions(&self) -> impl Iterator<Item = &Redaction> {
        match self {
            Self::Active { redactions } => redactions.iter(),
            Self::Noop => [].iter(),
        }
    }
}

/// An individual redaction to apply.
#[derive(Debug)]
enum Redaction {
    /// Redact a path.
    Path {
        /// The path to redact.
        path: Utf8PathBuf,

        /// The replacement string.
        replacement: String,
    },
}

fn build_linked_path_redactions<'a>(
    linked_paths: impl Iterator<Item = &'a Utf8Path>,
) -> BTreeMap<Utf8PathBuf, String> {
    // The map prevents dups.
    let mut linked_path_redactions = BTreeMap::new();

    for linked_path in linked_paths {
        // Linked paths are relative to the target dir, and usually of the form
        // <profile>/build/<crate-name>-<hash>/.... If the linked path matches this form, redact it
        // (in both absolute and relative forms).

        // First, look for a component of the form <crate-name>-hash in it.
        let mut source = Utf8PathBuf::new();
        let mut replacement = ReplacementBuilder::new();

        for elem in linked_path {
            if let Some(captures) = CRATE_NAME_HASH_REGEX.captures(elem) {
                // Found it! Redact it.
                let crate_name = captures.get(1).expect("regex had one capture");
                source.push(elem);
                replacement.push(&format!("<{}-hash>", crate_name.as_str()));
                linked_path_redactions.insert(source, replacement.into_string());
                break;
            } else {
                // Not found yet, keep looking.
                source.push(elem);
                replacement.push(elem);
            }

            // If the path isn't of the form above, we don't redact it.
        }
    }

    linked_path_redactions
}

#[derive(Debug)]
struct ReplacementBuilder {
    replacement: String,
}

impl ReplacementBuilder {
    fn new() -> Self {
        Self {
            replacement: String::new(),
        }
    }

    fn push(&mut self, s: &str) {
        if self.replacement.is_empty() {
            self.replacement.push_str(s);
        } else {
            self.replacement.push('/');
            self.replacement.push_str(s);
        }
    }

    fn into_string(self) -> String {
        self.replacement
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_path() {
        let abs_path = make_abs_path();
        let redactor = Redactor::new_with_kind(RedactorKind::Active {
            redactions: vec![
                Redaction::Path {
                    path: "target/debug".into(),
                    replacement: "<target-debug>".to_string(),
                },
                Redaction::Path {
                    path: "target".into(),
                    replacement: "<target-dir>".to_string(),
                },
                Redaction::Path {
                    path: abs_path.clone(),
                    replacement: "<abs-target>".to_string(),
                },
            ],
        });

        let examples: &[(Utf8PathBuf, &str)] = &[
            ("target/foo".into(), "<target-dir>/foo"),
            ("target/debug/bar".into(), "<target-debug>/bar"),
            ("target2/foo".into(), "target2/foo"),
            (
                // This will produce "<target-dir>/foo/bar" on Unix and "<target-dir>\\foo\\bar" on
                // Windows.
                ["target", "foo", "bar"].iter().collect(),
                "<target-dir>/foo/bar",
            ),
            (abs_path.clone(), "<abs-target>"),
            (abs_path.join("foo"), "<abs-target>/foo"),
        ];

        for (orig, expected) in examples {
            assert_eq!(
                redactor.redact_path(orig).to_string(),
                *expected,
                "redacting {orig:?}"
            );
        }
    }

    #[cfg(unix)]
    fn make_abs_path() -> Utf8PathBuf {
        "/path/to/target".into()
    }

    #[cfg(windows)]
    fn make_abs_path() -> Utf8PathBuf {
        "C:\\path\\to\\target".into()
        // TODO: test with verbatim paths
    }

    #[test]
    fn test_size_display() {
        insta::assert_snapshot!(SizeDisplay(0).to_string(), @"0 KB");
        insta::assert_snapshot!(SizeDisplay(512).to_string(), @"0 KB");
        insta::assert_snapshot!(SizeDisplay(1024).to_string(), @"1 KB");
        insta::assert_snapshot!(SizeDisplay(1536).to_string(), @"1 KB");
        insta::assert_snapshot!(SizeDisplay(10 * 1024).to_string(), @"10 KB");
        insta::assert_snapshot!(SizeDisplay(1024 * 1024 - 1).to_string(), @"1023 KB");

        insta::assert_snapshot!(SizeDisplay(1024 * 1024).to_string(), @"1.0 MB");
        insta::assert_snapshot!(SizeDisplay(1024 * 1024 + 512 * 1024).to_string(), @"1.5 MB");
        insta::assert_snapshot!(SizeDisplay(10 * 1024 * 1024).to_string(), @"10.0 MB");
        insta::assert_snapshot!(SizeDisplay(1024 * 1024 * 1024).to_string(), @"1024.0 MB");
    }
}
