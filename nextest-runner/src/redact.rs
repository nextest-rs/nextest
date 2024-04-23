// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Redact data that varies by system and OS to produce a stable output.
//!
//! Used for snapshot testing.

use crate::{
    helpers::{convert_rel_path_to_forward_slash, FormattedDuration},
    list::RustBuildMeta,
};
use camino::{Utf8Path, Utf8PathBuf};
use once_cell::sync::Lazy;
use regex::Regex;
use std::{collections::BTreeMap, fmt, sync::Arc, time::Duration};

static CRATE_NAME_HASH_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^([a-zA-Z0-9_-]+)-[a-f0-9]{16}$").unwrap());
static TARGET_DIR_REDACTION: &str = "<target-dir>";
static FILE_COUNT_REDACTION: &str = "<file-count>";
static DURATION_REDACTION: &str = "<duration>";

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
                replacement: format!("{}/{}", TARGET_DIR_REDACTION, replacement),
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
                            let path = Utf8PathBuf::from(format!("{}/{}", replacement, suffix));
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
///
/// Accepted by [`Redactor::new`].
#[derive(Debug)]
pub enum Redaction {
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
}
