// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for string-only writes.
//!
//! There are potential situations where nextest needs to abstract over multiple kinds of writers,
//! but some of them do not accept arbitrary bytes -- they must be valid UTF-8.
//!
//! This is similar to [`std::fmt::Write`], but it returns [`std::io::Error`] instead for better
//! error handling.

use std::{
    fmt,
    io::{self, BufWriter, Write},
};

/// A trait that abstracts over writing strings to a writer.
///
/// For more, see the [module-level documentation](self).
pub trait WriteStr {
    /// Writes a string to the writer.
    fn write_str(&mut self, s: &str) -> io::Result<()>;

    /// Flushes the writer, ensuring that all intermediately buffered contents reach their
    /// destination.
    fn write_str_flush(&mut self) -> io::Result<()>;

    /// Writes a single character to the writer.
    fn write_char(&mut self, c: char) -> io::Result<()> {
        self.write_str(c.encode_utf8(&mut [0; 4]))
    }

    /// Writes a formatted string to the writer.
    fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> io::Result<()> {
        // This code is adapted from the `write_fmt` implementation for `std::io::Write`, and is
        // used under the terms of the MIT and Apache-2.0 licenses.

        // Create a shim which translates self to a fmt::Write and saves off errors instead of
        // discarding them.
        struct Adapter<'a, T: ?Sized> {
            inner: &'a mut T,
            error: Result<(), io::Error>,
        }

        impl<T: ?Sized + WriteStr> fmt::Write for Adapter<'_, T> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                match self.inner.write_str(s) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        self.error = Err(e);
                        Err(fmt::Error)
                    }
                }
            }
        }

        let mut output = Adapter {
            inner: self,
            error: Ok(()),
        };
        match fmt::write(&mut output, fmt) {
            Ok(()) => Ok(()),
            Err(_) => {
                // check if the error came from the underlying `Write` or not
                if output.error.is_err() {
                    output.error
                } else {
                    Err(io::Error::other("formatter error"))
                }
            }
        }
    }
}

impl WriteStr for String {
    fn write_str(&mut self, s: &str) -> io::Result<()> {
        self.push_str(s);
        Ok(())
    }

    fn write_str_flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write_char(&mut self, c: char) -> io::Result<()> {
        self.push(c);
        Ok(())
    }
}

impl<W: Write> WriteStr for BufWriter<W> {
    fn write_str(&mut self, s: &str) -> io::Result<()> {
        self.write_all(s.as_bytes())
    }

    fn write_str_flush(&mut self) -> io::Result<()> {
        self.flush()
    }

    fn write_char(&mut self, c: char) -> io::Result<()> {
        self.write_all(c.encode_utf8(&mut [0; 4]).as_bytes())
    }
}

impl<T: WriteStr + ?Sized> WriteStr for &mut T {
    fn write_str(&mut self, s: &str) -> io::Result<()> {
        (**self).write_str(s)
    }

    fn write_str_flush(&mut self) -> io::Result<()> {
        (**self).write_str_flush()
    }

    fn write_char(&mut self, c: char) -> io::Result<()> {
        (**self).write_char(c)
    }

    fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> io::Result<()> {
        (**self).write_fmt(fmt)
    }
}

// Add more impls as needed.
