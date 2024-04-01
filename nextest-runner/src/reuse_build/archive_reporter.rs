// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::helpers::{format_duration, plural};
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
    // TODO: message-format json?
}

impl ArchiveReporter {
    /// Creates a new reporter for archive events.
    pub fn new(verbose: bool) -> Self {
        Self {
            styles: Styles::default(),
            verbose,
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
                test_binary_count,
                non_test_binary_count,
                build_script_out_dir_count,
                linked_path_count,
                output_file,
            } => {
                write!(writer, "{:>12} ", "Archiving".style(self.styles.success))?;

                self.report_binary_counts(
                    test_binary_count,
                    non_test_binary_count,
                    build_script_out_dir_count,
                    linked_path_count,
                    &mut writer,
                )?;

                writeln!(writer, " to {}", output_file.style(self.styles.bold))?;
            }
            ArchiveEvent::RecursionDepthExceeded { path, limit, warn } => {
                if warn {
                    write!(writer, "{:>12} ", "Warning".style(self.styles.warning))?;
                } else if self.verbose {
                    write!(writer, "{:>12} ", "Skipped".style(self.styles.skipped))?;
                } else {
                    return Ok(()); // Skip
                }

                writeln!(
                    writer,
                    "recursion depth exceeded at {} (limit: {limit})",
                    path.style(self.styles.bold),
                )?;
            }
            ArchiveEvent::UnknownFileType { path } => {
                write!(writer, "{:>12} ", "Warning".style(self.styles.warning))?;
                writeln!(
                    writer,
                    "ignoring `{}` because it is not a file, \
                     directory, or symbolic link",
                    path.style(self.styles.bold),
                )?;
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
                    file_count.style(self.styles.bold),
                    output_file.style(self.styles.bold),
                    format_duration(elapsed),
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

                self.report_binary_counts(
                    test_binary_count,
                    non_test_binary_count,
                    build_script_out_dir_count,
                    linked_path_count,
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
                    file_count.style(self.styles.bold),
                    plural::files_str(file_count),
                    destination_dir.style(self.styles.bold),
                    format_duration(elapsed),
                )?;
            }
        }

        Ok(())
    }

    fn report_binary_counts(
        &mut self,
        test_binary_count: usize,
        non_test_binary_count: usize,
        build_script_out_dir_count: usize,
        linked_path_count: usize,
        mut writer: impl Write,
    ) -> io::Result<()> {
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
        /// The number of test binaries to archive.
        test_binary_count: usize,

        /// The number of non-test binaries to archive.
        non_test_binary_count: usize,

        /// The number of build script output directories to archive.
        build_script_out_dir_count: usize,

        /// The number of linked paths to archive.
        linked_path_count: usize,

        /// The archive output file.
        output_file: &'a Utf8Path,
    },

    /// While performing the archive, the recursion depth was exceeded.
    RecursionDepthExceeded {
        /// The path that exceeded the recursion depth.
        path: &'a Utf8Path,

        /// The recursion depth limit that was hit.
        limit: usize,

        /// Whether the reporter should produce a warning about this.
        warn: bool,
    },

    /// The archive process encountered an unknown file type.
    UnknownFileType {
        /// The path of the unknown type.
        path: &'a Utf8Path,
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
