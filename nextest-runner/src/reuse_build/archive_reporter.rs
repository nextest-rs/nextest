// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8Path;
use owo_colors::{OwoColorize, Style};
use std::{
    io::{self, Write},
    time::Duration,
};

use crate::helpers::format_duration;

#[derive(Debug)]
/// Reporter for archive operations.
pub struct ArchiveReporter {
    styles: Styles,
    // TODO: message-format json?
}

impl ArchiveReporter {
    /// Creates a new reporter for archive events.
    pub fn new(_verbose: bool) -> Self {
        Self {
            styles: Styles::default(),
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
                linked_path_count,
                output_file,
            } => {
                write!(writer, "{:>12} ", "Archiving".style(self.styles.success))?;

                self.report_binary_counts(
                    test_binary_count,
                    non_test_binary_count,
                    linked_path_count,
                    &mut writer,
                )?;

                writeln!(writer, " to {}", output_file.style(self.styles.bold))?;
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
                file_count,
                test_binary_count,
                non_test_binary_count,
                linked_path_count,
                dest_dir: destination_dir,
            } => {
                write!(writer, "{:>12} ", "Extracting".style(self.styles.success))?;

                self.report_binary_counts(
                    test_binary_count,
                    non_test_binary_count,
                    linked_path_count,
                    &mut writer,
                )?;

                writeln!(
                    writer,
                    " ({} files total) to {}",
                    file_count.style(self.styles.bold),
                    destination_dir.style(self.styles.bold),
                )?;
            }
            ArchiveEvent::Extracted {
                file_count,
                dest_dir: destination_dir,
                elapsed,
            } => {
                write!(writer, "{:>12} ", "Extracted".style(self.styles.success))?;
                writeln!(
                    writer,
                    "{} files to {} in {}",
                    file_count.style(self.styles.bold),
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
        linked_path_count: usize,
        mut writer: impl Write,
    ) -> io::Result<()> {
        let total_binary_count = test_binary_count + non_test_binary_count;
        let non_test_text = if non_test_binary_count > 0 {
            format!(
                " (including {} non-test binaries)",
                non_test_binary_count.style(self.styles.bold)
            )
        } else {
            "".to_owned()
        };
        let linked_path_text = if linked_path_count > 0 {
            format!(
                " and {} linked paths",
                linked_path_count.style(self.styles.bold)
            )
        } else {
            "".to_owned()
        };

        write!(
            writer,
            "{} binaries{non_test_text}{linked_path_text}",
            total_binary_count.style(self.styles.bold),
        )
    }
}

#[derive(Debug, Default)]
struct Styles {
    bold: Style,
    success: Style,
}

impl Styles {
    fn colorize(&mut self) {
        self.bold = Style::new().bold();
        self.success = Style::new().green().bold();
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

        /// The number of linked paths to archive.
        linked_path_count: usize,

        /// The archive output file.
        output_file: &'a Utf8Path,
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
        /// The number of files in the archive.
        file_count: usize,

        /// The number of test binaries to extract.
        test_binary_count: usize,

        /// The number of non-test binaries to extract.
        non_test_binary_count: usize,

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
