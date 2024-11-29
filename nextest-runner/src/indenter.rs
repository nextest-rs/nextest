// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for indenting multi-line displays.
//!
//! This module is adapted from [indenter](https://github.com/eyre-rs/indenter) and is used under the
//! terms of the MIT or Apache-2.0 licenses.
//!
//! # Notes
//!
//! We previously used to use `indenter` to indent multi-line `fmt::Display`
//! instances. However, at some point we needed to also indent
//! `std::io::Write`s, not just `fmt::Write`. `indenter` 0.3.3 doesn't support
//! `std::io::Write`, so we switched to `indent_write`.
//!
//! This file still has the `indenter` API. So we have both APIs floating around
//! for a bit... oh well. Still in two minds about which one's better here.

use crate::write_str::WriteStr;
use std::io;

/// The set of supported formats for indentation
#[expect(missing_debug_implementations)]
pub enum Format<'a> {
    /// Insert uniform indentation before every line
    ///
    /// This format takes a static string as input and inserts it after every newline
    Uniform {
        /// The string to insert as indentation
        indentation: &'static str,
    },
    /// Inserts a number before the first line
    ///
    /// This format hard codes the indentation level to match the indentation from
    /// `core::backtrace::Backtrace`
    Numbered {
        /// The index to insert before the first line of output
        ind: usize,
    },
    /// A custom indenter which is executed after every newline
    ///
    /// Custom indenters are passed the current line number and the buffer to be written to as args
    Custom {
        /// The custom indenter
        inserter: &'a mut Inserter,
    },
}

/// Helper struct for efficiently indenting multi line display implementations
///
/// # Explanation
///
/// This type will never allocate a string to handle inserting indentation. It instead leverages
/// the `write_str` function that serves as the foundation of the `core::fmt::Write` trait. This
/// lets it intercept each piece of output as its being written to the output buffer. It then
/// splits on newlines giving slices into the original string. Finally we alternate writing these
/// lines and the specified indentation to the output buffer.
#[expect(missing_debug_implementations)]
pub struct Indented<'a, D: ?Sized> {
    inner: &'a mut D,
    needs_indent: bool,
    format: Format<'a>,
}

/// A callback for `Format::Custom` used to insert indenation after a new line
///
/// The first argument is the line number within the output, starting from 0
pub type Inserter = dyn FnMut(usize, &mut dyn WriteStr) -> io::Result<()>;

impl Format<'_> {
    fn insert_indentation(&mut self, line: usize, f: &mut dyn WriteStr) -> io::Result<()> {
        match self {
            Format::Uniform { indentation } => write!(f, "{indentation}"),
            Format::Numbered { ind } => {
                if line == 0 {
                    write!(f, "{ind: >4}: ")
                } else {
                    write!(f, "      ")
                }
            }
            Format::Custom { inserter } => inserter(line, f),
        }
    }
}

impl<'a, D: ?Sized> Indented<'a, D> {
    /// Sets the format to `Format::Numbered` with the provided index
    pub fn ind(self, ind: usize) -> Self {
        self.with_format(Format::Numbered { ind })
    }

    /// Sets the format to `Format::Uniform` with the provided static string
    pub fn with_str(self, indentation: &'static str) -> Self {
        self.with_format(Format::Uniform { indentation })
    }

    /// Construct an indenter with a user defined format
    pub fn with_format(mut self, format: Format<'a>) -> Self {
        self.format = format;
        self
    }

    /// Returns the inner writer.
    pub fn into_inner(self) -> &'a mut D {
        self.inner
    }
}

impl<T> WriteStr for Indented<'_, T>
where
    T: WriteStr + ?Sized,
{
    fn write_str(&mut self, s: &str) -> io::Result<()> {
        for (ind, line) in s.split('\n').enumerate() {
            if ind > 0 {
                self.inner.write_char('\n')?;
                self.needs_indent = true;
            }

            if self.needs_indent {
                // Don't render the line unless its actually got text on it
                if line.is_empty() {
                    continue;
                }

                self.format.insert_indentation(ind, &mut self.inner)?;
                self.needs_indent = false;
            }

            self.inner.write_fmt(format_args!("{line}"))?;
        }

        Ok(())
    }

    fn write_str_flush(&mut self) -> io::Result<()> {
        // We don't need to do any flushing ourselves, because there's no intermediate state
        // possible here.
        self.inner.write_str_flush()
    }
}

/// Helper function for creating a default indenter
pub fn indented<D: ?Sized>(f: &mut D) -> Indented<'_, D> {
    Indented {
        inner: f,
        needs_indent: true,
        format: Format::Uniform {
            indentation: "    ",
        },
    }
}
