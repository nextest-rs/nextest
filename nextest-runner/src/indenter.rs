// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for indenting multi-line displays.
//!
//! This module is adapted from [indenter](https://github.com/eyre-rs/indenter) and is used under
//! the terms of the MIT or Apache-2.0 licenses.
//!
//! The main type is [`Indented`], which wraps a writer and indents each line. It works with both
//! [`WriteStr`] and [`fmt::Write`].

use crate::write_str::WriteStr;
use std::{
    fmt::{self, Write as _},
    io,
};

/// Helper struct for efficiently indenting multi-line display implementations.
///
/// This type will never allocate a string to handle inserting indentation. It instead leverages
/// the `write_str` function that serves as the foundation of the `core::fmt::Write` trait. This
/// lets it intercept each piece of output as it's being written to the output buffer. It then
/// splits on newlines giving slices into the original string. Finally we alternate writing these
/// lines and the specified indentation to the output buffer.
#[expect(missing_debug_implementations)]
pub struct Indented<'a, D: ?Sized> {
    inner: &'a mut D,
    needs_indent: bool,
    indentation: &'static str,
}

impl<'a, D: ?Sized> Indented<'a, D> {
    /// Sets the indentation string.
    pub fn with_str(mut self, indentation: &'static str) -> Self {
        self.indentation = indentation;
        self
    }

    /// Skip indenting the initial line.
    ///
    /// This is useful when you've already written some content on the current line
    /// and want to start indenting only from the next line onwards.
    pub fn skip_initial(mut self) -> Self {
        self.needs_indent = false;
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
                // Don't render the line unless it actually has text on it.
                if line.is_empty() {
                    continue;
                }

                self.inner.write_str(self.indentation)?;
                self.needs_indent = false;
            }

            self.inner.write_str(line)?;
        }

        Ok(())
    }

    fn write_str_flush(&mut self) -> io::Result<()> {
        // We don't need to do any flushing ourselves, because there's no intermediate state
        // possible here.
        self.inner.write_str_flush()
    }
}

impl<T> fmt::Write for Indented<'_, T>
where
    T: fmt::Write + ?Sized,
{
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for (ind, line) in s.split('\n').enumerate() {
            if ind > 0 {
                self.inner.write_char('\n')?;
                self.needs_indent = true;
            }

            if self.needs_indent {
                // Don't render the line unless it actually has text on it.
                if line.is_empty() {
                    continue;
                }

                self.inner.write_str(self.indentation)?;
                self.needs_indent = false;
            }

            self.inner.write_str(line)?;
        }

        Ok(())
    }
}

/// Helper function for creating a default indenter.
pub fn indented<D: ?Sized>(f: &mut D) -> Indented<'_, D> {
    Indented {
        inner: f,
        needs_indent: true,
        indentation: "    ",
    }
}

/// Wraps a `Display` item to indent each line when displayed.
///
/// This is useful for indenting error messages or other multi-line output.
#[derive(Clone, Debug)]
pub struct DisplayIndented<T> {
    /// The item to display with indentation.
    pub item: T,
    /// The indentation string to prepend to each line.
    pub indent: &'static str,
}

impl<T: fmt::Display> fmt::Display for DisplayIndented<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut indented = Indented {
            inner: f,
            needs_indent: true,
            indentation: self.indent,
        };
        write!(indented, "{}", self.item)
    }
}
