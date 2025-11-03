// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use bstr::ByteSlice;
use owo_colors::Style;

/// Given a slice, find the index of the point at which highlighting should end.
///
/// Returns a value in the range [0, slice.len()].
pub fn highlight_end(slice: &[u8]) -> usize {
    // We want to highlight the first two lines of the output.
    let mut iter = slice.find_iter(b"\n");
    match iter.next() {
        Some(_) => {
            match iter.next() {
                Some(second) => second,
                // No second newline found, so highlight the entire slice.
                None => slice.len(),
            }
        }
        // No newline found, so highlight the entire slice.
        None => slice.len(),
    }
}

#[derive(Debug, Default, Clone)]
pub(super) struct Styles {
    pub(super) is_colorized: bool,
    pub(super) count: Style,
    pub(super) pass: Style,
    pub(super) retry: Style,
    pub(super) fail: Style,
    pub(super) skip: Style,
    pub(super) script_id: Style,
    pub(super) list_styles: crate::list::Styles,
}

impl Styles {
    pub(super) fn colorize(&mut self) {
        self.is_colorized = true;
        self.count = Style::new().bold();
        self.pass = Style::new().green().bold();
        self.retry = Style::new().magenta().bold();
        self.fail = Style::new().red().bold();
        self.skip = Style::new().yellow().bold();
        self.script_id = Style::new().blue().bold();
        self.list_styles.colorize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_end() {
        let tests: &[(&str, usize)] = &[
            ("", 0),
            ("\n", 1),
            ("foo", 3),
            ("foo\n", 4),
            ("foo\nbar", 7),
            ("foo\nbar\n", 7),
            ("foo\nbar\nbaz", 7),
            ("foo\nbar\nbaz\n", 7),
            ("\nfoo\nbar\nbaz", 4),
        ];

        for (input, output) in tests {
            assert_eq!(
                highlight_end(input.as_bytes()),
                *output,
                "for input {input:?}"
            );
        }
    }
}
