// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::ArchiveStep;
use crate::{helpers::plural, redact::Redactor};
use camino::Utf8Path;
use owo_colors::{OwoColorize, Style};
use std::{
    io::{self, Write},
    time::Duration,
};

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
        mut writer: impl Write,
    ) -> io::Result<()> {
        match event {
            ArchiveEvent::ArchiveStarted {
                counts,
                output_file,
            } => {
                write!(writer, "{:>12} ", "Archiving".style(self.styles.success))?;

                self.report_counts(counts, &mut writer)?;

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
                    "could not find standard library for host (proc macro tests may not work): {}",
                    error
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
                        non_test_binary_count,
                        build_script_out_dir_count,
                        linked_path_count,
                        // TODO: we currently don't store a list of extra paths or standard libs at
                        // manifest creation time, so we can't report this count here.
                        extra_path_count: 0,
                        stdlib_count: 0,
                    },
                    &mut writer,
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

    fn report_counts(&mut self, counts: ArchiveCounts, mut writer: impl Write) -> io::Result<()> {
        let ArchiveCounts {
            test_binary_count,
            non_test_binary_count,
            build_script_out_dir_count,
            linked_path_count,
            extra_path_count,
            stdlib_count,
        } = counts;

        let total_binary_count = test_binary_count + non_test_binary_count;
        let non_test_text = if non_test_binary_count > 0 {
            format!(
                " (including {} non-test {})",
                non_test_binary_count.style(self.styles.bold),
                plural::binaries_str(non_test_binary_count),
            )
        } else {
            "".to_owned()
        };
        let mut more = Vec::new();
        if build_script_out_dir_count > 0 {
            more.push(format!(
                "{} build script output {}",
                build_script_out_dir_count.style(self.styles.bold),
                plural::directories_str(build_script_out_dir_count),
            ));
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

        write!(
            writer,
            "{} {}{non_test_text}",
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
#[derive(Clone, Copy, Debug)]
pub struct ArchiveCounts {
    /// The number of test binaries.
    pub test_binary_count: usize,

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
