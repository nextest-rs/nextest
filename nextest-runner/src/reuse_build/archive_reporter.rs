// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::ArchiveStep;
use crate::{helpers::plural, redact::Redactor, write_str::WriteStr};
use camino::Utf8Path;
use owo_colors::{OwoColorize, Style};
use std::{io, time::Duration};
use swrite::{SWrite, swrite};

#[derive(Debug)]
/// Reporter for archive operations.
pub struct ArchiveReporter {
    styles: Styles,
    verbose: bool,
    redactor: Redactor,

    linked_path_hint_emitted: bool,
    // TODO: message-format json?
}

impl ArchiveReporter {
    /// Creates a new reporter for archive events.
    pub fn new(verbose: bool, redactor: Redactor) -> Self {
        Self {
            styles: Styles::default(),
            verbose,
            redactor,

            linked_path_hint_emitted: false,
        }
    }

    /// Colorizes output.
    pub fn colorize(&mut self) {
        self.styles.colorize();
    }

    /// Reports an archive event.
    pub fn report_event(
        &mut self,
        event: ArchiveEvent<'_>,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        match event {
            ArchiveEvent::ArchiveStarted {
                counts,
                output_file,
            } => {
                write!(writer, "{:>12} ", "Archiving".style(self.styles.success))?;

                self.report_counts(counts, writer)?;

                writeln!(
                    writer,
                    " to {}",
                    self.redactor
                        .redact_path(output_file)
                        .style(self.styles.bold)
                )?;
            }
            ArchiveEvent::StdlibPathError { error } => {
                write!(writer, "{:>12} ", "Warning".style(self.styles.bold))?;
                writeln!(
                    writer,
                    "could not find standard library for host (proc macro tests may not work): {error}"
                )?;
            }
            ArchiveEvent::ExtraPathMissing { path, warn } => {
                if warn {
                    write!(writer, "{:>12} ", "Warning".style(self.styles.warning))?;
                } else if self.verbose {
                    write!(writer, "{:>12} ", "Skipped".style(self.styles.skipped))?;
                } else {
                    return Ok(()); // Skip
                }

                writeln!(
                    writer,
                    "ignoring extra path `{}` because it does not exist",
                    self.redactor.redact_path(path).style(self.styles.bold),
                )?;
            }
            ArchiveEvent::DirectoryAtDepthZero { path } => {
                write!(writer, "{:>12} ", "Warning".style(self.styles.warning))?;
                writeln!(
                    writer,
                    "ignoring extra path `{}` specified with depth 0 since it is a directory",
                    self.redactor.redact_path(path).style(self.styles.bold),
                )?;
            }
            ArchiveEvent::RecursionDepthExceeded {
                step,
                path,
                limit,
                warn,
            } => {
                if warn {
                    write!(writer, "{:>12} ", "Warning".style(self.styles.warning))?;
                } else if self.verbose {
                    write!(writer, "{:>12} ", "Skipped".style(self.styles.skipped))?;
                } else {
                    return Ok(()); // Skip
                }

                writeln!(
                    writer,
                    "while archiving {step}, recursion depth exceeded at {} (limit: {limit})",
                    self.redactor.redact_path(path).style(self.styles.bold),
                )?;
            }
            ArchiveEvent::UnknownFileType { step, path } => {
                write!(writer, "{:>12} ", "Warning".style(self.styles.warning))?;
                writeln!(
                    writer,
                    "while archiving {step}, ignoring `{}` because it is not a file, \
                     directory, or symbolic link",
                    self.redactor.redact_path(path).style(self.styles.bold),
                )?;
            }
            ArchiveEvent::LinkedPathNotFound { path, requested_by } => {
                write!(writer, "{:>12} ", "Warning".style(self.styles.warning))?;
                writeln!(
                    writer,
                    "linked path `{}` not found, requested by: {}",
                    self.redactor.redact_path(path).style(self.styles.bold),
                    requested_by.join(", ").style(self.styles.bold),
                )?;
                if !self.linked_path_hint_emitted {
                    write!(writer, "{:>12} ", "")?;
                    writeln!(
                        writer,
                        "(this is a bug in {} that should be fixed)",
                        plural::this_crate_str(requested_by.len())
                    )?;
                    self.linked_path_hint_emitted = true;
                }
            }
            ArchiveEvent::Archived {
                file_count,
                output_file,
                elapsed,
            } => {
                write!(writer, "{:>12} ", "Archived".style(self.styles.success))?;
                writeln!(
                    writer,
                    "{} files to {} in {}",
                    self.redactor
                        .redact_file_count(file_count)
                        .style(self.styles.bold),
                    self.redactor
                        .redact_path(output_file)
                        .style(self.styles.bold),
                    self.redactor.redact_duration(elapsed),
                )?;
            }
            ArchiveEvent::ExtractStarted {
                test_binary_count,
                non_test_binary_count,
                build_script_out_dir_count,
                linked_path_count,
                dest_dir: destination_dir,
            } => {
                write!(writer, "{:>12} ", "Extracting".style(self.styles.success))?;

                self.report_counts(
                    ArchiveCounts {
                        test_binary_count,
                        // We don't track filtered-out test binaries during
                        // extraction.
                        filter_counts: ArchiveFilterCounts::default(),
                        non_test_binary_count,
                        build_script_out_dir_count,
                        linked_path_count,
                        // TODO: we currently don't store a list of extra paths or standard libs at
                        // manifest creation time, so we can't report this count here.
                        extra_path_count: 0,
                        stdlib_count: 0,
                    },
                    writer,
                )?;

                writeln!(writer, " to {}", destination_dir.style(self.styles.bold))?;
            }
            ArchiveEvent::Extracted {
                file_count,
                dest_dir: destination_dir,
                elapsed,
            } => {
                write!(writer, "{:>12} ", "Extracted".style(self.styles.success))?;
                writeln!(
                    writer,
                    "{} {} to {} in {}",
                    self.redactor
                        .redact_file_count(file_count)
                        .style(self.styles.bold),
                    plural::files_str(file_count),
                    self.redactor
                        .redact_path(destination_dir)
                        .style(self.styles.bold),
                    self.redactor.redact_duration(elapsed),
                )?;
            }
        }

        Ok(())
    }

    fn report_counts(
        &mut self,
        counts: ArchiveCounts,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        let ArchiveCounts {
            test_binary_count,
            filter_counts:
                ArchiveFilterCounts {
                    filtered_out_test_binary_count,
                    filtered_out_non_test_binary_count,
                    filtered_out_build_script_out_dir_count,
                },
            non_test_binary_count,
            build_script_out_dir_count,
            linked_path_count,
            extra_path_count,
            stdlib_count,
        } = counts;

        let total_binary_count = test_binary_count + non_test_binary_count;
        let mut in_parens = Vec::new();
        if non_test_binary_count > 0 {
            in_parens.push(format!(
                "including {} non-test {}",
                non_test_binary_count.style(self.styles.bold),
                plural::binaries_str(non_test_binary_count),
            ));
        }
        if filtered_out_test_binary_count > 0 || filtered_out_non_test_binary_count > 0 {
            let mut filtered_out = Vec::new();
            if filtered_out_test_binary_count > 0 {
                filtered_out.push(format!(
                    "{} test {}",
                    filtered_out_test_binary_count.style(self.styles.bold),
                    plural::binaries_str(filtered_out_test_binary_count),
                ));
            }
            if filtered_out_non_test_binary_count > 0 {
                filtered_out.push(format!(
                    "{} non-test {}",
                    filtered_out_non_test_binary_count.style(self.styles.bold),
                    plural::binaries_str(filtered_out_non_test_binary_count),
                ));
            }

            in_parens.push(format!("{} filtered out", filtered_out.join(" and ")));
        }
        let mut more = Vec::new();
        if build_script_out_dir_count > 0 {
            let mut s = format!(
                "{} build script output {}",
                build_script_out_dir_count.style(self.styles.bold),
                plural::directories_str(build_script_out_dir_count),
            );
            if filtered_out_build_script_out_dir_count > 0 {
                swrite!(
                    s,
                    " ({} filtered out)",
                    filtered_out_build_script_out_dir_count.style(self.styles.bold)
                );
            }
            more.push(s);
        }
        if linked_path_count > 0 {
            more.push(format!(
                "{} linked {}",
                linked_path_count.style(self.styles.bold),
                plural::paths_str(linked_path_count),
            ));
        }
        if extra_path_count > 0 {
            more.push(format!(
                "{} extra {}",
                extra_path_count.style(self.styles.bold),
                plural::paths_str(extra_path_count),
            ));
        }
        if stdlib_count > 0 {
            more.push(format!(
                "{} standard {}",
                stdlib_count.style(self.styles.bold),
                plural::libraries_str(stdlib_count),
            ));
        }

        let parens_text = if in_parens.is_empty() {
            String::new()
        } else {
            format!(" ({})", in_parens.join("; "))
        };

        write!(
            writer,
            "{} {}{parens_text}",
            total_binary_count.style(self.styles.bold),
            plural::binaries_str(total_binary_count),
        )?;

        match more.len() {
            0 => Ok(()),
            1 => {
                write!(writer, " and {}", more[0])
            }
            _ => {
                write!(
                    writer,
                    ", {}, and {}",
                    more[..more.len() - 1].join(", "),
                    more.last().unwrap(),
                )
            }
        }
    }
}

#[derive(Debug, Default)]
struct Styles {
    bold: Style,
    success: Style,
    warning: Style,
    skipped: Style,
}

impl Styles {
    fn colorize(&mut self) {
        self.bold = Style::new().bold();
        self.success = Style::new().green().bold();
        self.warning = Style::new().yellow().bold();
        self.skipped = Style::new().bold();
    }
}

/// An archive event.
///
/// Events are produced by archive and extract operations, and consumed by an [`ArchiveReporter`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ArchiveEvent<'a> {
    /// The archive process started.
    ArchiveStarted {
        /// File counts.
        counts: ArchiveCounts,

        /// The archive output file.
        output_file: &'a Utf8Path,
    },

    /// An error occurred while obtaining the path to a standard library.
    StdlibPathError {
        /// The error that occurred.
        error: &'a str,
    },

    /// A provided extra path did not exist.
    ExtraPathMissing {
        /// The path that was missing.
        path: &'a Utf8Path,

        /// Whether the reporter should produce a warning about this.
        warn: bool,
    },

    /// For an extra include, a directory was specified at depth 0.
    DirectoryAtDepthZero {
        /// The directory that was at depth 0.
        path: &'a Utf8Path,
    },

    /// While performing the archive, the recursion depth was exceeded.
    RecursionDepthExceeded {
        /// The current step in the archive process.
        step: ArchiveStep,

        /// The path that exceeded the recursion depth.
        path: &'a Utf8Path,

        /// The recursion depth limit that was hit.
        limit: usize,

        /// Whether the reporter should produce a warning about this.
        warn: bool,
    },

    /// The archive process encountered an unknown file type.
    UnknownFileType {
        /// The current step in the archive process.
        step: ArchiveStep,

        /// The path of the unknown type.
        path: &'a Utf8Path,
    },

    /// A crate linked against a non-existent path.
    LinkedPathNotFound {
        /// The path of the linked file.
        path: &'a Utf8Path,

        /// The crates that linked against the path.
        requested_by: &'a [String],
    },

    /// The archive operation completed successfully.
    Archived {
        /// The number of files archived.
        file_count: usize,

        /// The archive output file.
        output_file: &'a Utf8Path,

        /// How long it took to create the archive.
        elapsed: Duration,
    },

    /// The extraction process started.
    ExtractStarted {
        /// The number of test binaries to extract.
        test_binary_count: usize,

        /// The number of non-test binaries to extract.
        non_test_binary_count: usize,

        /// The number of build script output directories to archive.
        build_script_out_dir_count: usize,

        /// The number of linked paths to extract.
        linked_path_count: usize,

        /// The destination directory.
        dest_dir: &'a Utf8Path,
    },

    /// The extraction process completed successfully.
    Extracted {
        /// The number of files extracted.
        file_count: usize,

        /// The destination directory.
        dest_dir: &'a Utf8Path,

        /// How long it took to extract the archive.
        elapsed: Duration,
    },
}

/// Counts of various types of files in an archive.
#[derive(Clone, Copy, Debug, Default)]
pub struct ArchiveCounts {
    /// The number of test binaries that will be included in the archive, not
    /// including filtered out test binaries.
    pub test_binary_count: usize,

    /// Counts for filtered out binaries.
    pub filter_counts: ArchiveFilterCounts,

    /// The number of non-test binaries.
    pub non_test_binary_count: usize,

    /// The number of build script output directories.
    pub build_script_out_dir_count: usize,

    /// The number of linked paths.
    pub linked_path_count: usize,

    /// The number of extra paths.
    pub extra_path_count: usize,

    /// The number of standard libraries.
    pub stdlib_count: usize,
}

/// Counts the number of filtered out binaries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArchiveFilterCounts {
    /// The number of filtered out test binaries.
    pub filtered_out_test_binary_count: usize,

    /// The number of filtered out non-test binaries.
    pub filtered_out_non_test_binary_count: usize,

    /// The number of filtered out build script output directories.
    pub filtered_out_build_script_out_dir_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(
        ArchiveCounts {
            test_binary_count: 1,
            ..Default::default()
        },
        "1 binary"
        ; "single test binary"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 5,
            ..Default::default()
        },
        "5 binaries"
        ; "multiple test binaries"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 5,
            non_test_binary_count: 2,
            ..Default::default()
        },
        "7 binaries (including 2 non-test binaries)"
        ; "with non-test binaries"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 5,
            filter_counts: ArchiveFilterCounts {
                filtered_out_test_binary_count: 2,
                ..Default::default()
            },
            ..Default::default()
        },
        "5 binaries (2 test binaries filtered out)"
        ; "with filtered out test binaries"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 5,
            filter_counts: ArchiveFilterCounts {
                filtered_out_non_test_binary_count: 1,
                ..Default::default()
            },
            ..Default::default()
        },
        "5 binaries (1 non-test binary filtered out)"
        ; "with filtered out non-test binary"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 5,
            filter_counts: ArchiveFilterCounts {
                filtered_out_test_binary_count: 2,
                filtered_out_non_test_binary_count: 3,
                ..Default::default()
            },
            ..Default::default()
        },
        "5 binaries (2 test binaries and 3 non-test binaries filtered out)"
        ; "with both types filtered out"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 5,
            filter_counts: ArchiveFilterCounts {
                filtered_out_test_binary_count: 1,
                filtered_out_non_test_binary_count: 2,
                ..Default::default()
            },
            non_test_binary_count: 3,
            ..Default::default()
        },
        "8 binaries (including 3 non-test binaries; 1 test binary and 2 non-test binaries filtered out)"
        ; "with non-test binaries and both types filtered out"
    )]
    #[test_case(
        ArchiveCounts {
            filter_counts: ArchiveFilterCounts {
                filtered_out_non_test_binary_count: 2,
                ..Default::default()
            },
            ..Default::default()
        },
        "0 binaries (2 non-test binaries filtered out)"
        ; "zero binaries with filtered out"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 5,
            build_script_out_dir_count: 1,
            ..Default::default()
        },
        "5 binaries and 1 build script output directory"
        ; "with single more item"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 5,
            build_script_out_dir_count: 3,
            filter_counts: ArchiveFilterCounts {
                filtered_out_build_script_out_dir_count: 2,
                ..Default::default()
            },
            ..Default::default()
        },
        "5 binaries and 3 build script output directories (2 filtered out)"
        ; "with filtered out build script out dirs"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 5,
            linked_path_count: 3,
            ..Default::default()
        },
        "5 binaries and 3 linked paths"
        ; "with linked paths"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 5,
            build_script_out_dir_count: 2,
            linked_path_count: 3,
            extra_path_count: 1,
            stdlib_count: 4,
            ..Default::default()
        },
        "5 binaries, 2 build script output directories, 3 linked paths, 1 extra path, and 4 standard libraries"
        ; "with multiple more items"
    )]
    #[test_case(
        ArchiveCounts {
            test_binary_count: 4,
            filter_counts: ArchiveFilterCounts {
                filtered_out_test_binary_count: 1,
                filtered_out_non_test_binary_count: 2,
                filtered_out_build_script_out_dir_count: 1,
            },
            non_test_binary_count: 2,
            build_script_out_dir_count: 3,
            linked_path_count: 2,
            extra_path_count: 1,
            stdlib_count: 2,
        },
        "6 binaries (including 2 non-test binaries; 1 test binary and 2 non-test binaries filtered out), 3 build script output directories (1 filtered out), 2 linked paths, 1 extra path, and 2 standard libraries"
        ; "all fields combined"
    )]
    #[test_case(
        ArchiveCounts::default(),
        "0 binaries"
        ; "all zeros"
    )]
    fn test_report_counts(counts: ArchiveCounts, expected: &str) {
        let mut reporter = ArchiveReporter::new(false, Redactor::noop());
        let mut buffer = String::new();

        reporter.report_counts(counts, &mut buffer).unwrap();

        assert_eq!(buffer, expected);
    }
}
