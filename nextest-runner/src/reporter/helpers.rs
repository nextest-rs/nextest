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
    /// Style for the unique prefix portion of run IDs (highlighted).
    pub(super) run_id_prefix: Style,
    /// Style for the non-unique rest portion of run IDs (dimmed).
    pub(super) run_id_rest: Style,
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
        self.run_id_prefix = Style::new().bold().purple();
        self.run_id_rest = Style::new().bright_black();
    }
}

// Port of std::str::floor_char_boundary to Rust < 1.91.0. Remove after MSRV
// has been bumped to 1.91 or above.
pub(crate) const fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else {
        let mut i = index;
        while i > 0 {
            if is_utf8_char_boundary(s.as_bytes()[i]) {
                break;
            }
            i -= 1;
        }

        //  The character boundary will be within four bytes of the index
        debug_assert!(i >= index.saturating_sub(3));

        i
    }
}

#[inline]
const fn is_utf8_char_boundary(b: u8) -> bool {
    // This is bit magic equivalent to: b < 128 || b >= 192
    (b as i8) >= -0x40
}

/// Calls the provided callback with chunks of text, breaking at newline
/// boundaries when possible.
///
/// This function processes text in chunks to avoid performance issues when
/// printing large amounts of text at once. It attempts to break chunks at
/// newline boundaries within the specified maximum chunk size, but will
/// handle lines longer than the maximum by searching forward for the next
/// newline.
///
/// # Parameters
/// - `text`: The text to process
/// - `max_chunk_bytes`: Maximum size of each chunk in bytes
/// - `callback`: Function called with each chunk of text (includes trailing newlines)
pub(crate) fn print_lines_in_chunks(
    text: &str,
    max_chunk_bytes: usize,
    mut callback: impl FnMut(&str),
) {
    let mut remaining = text;
    while !remaining.is_empty() {
        // Find the maximum index to search for the last newline, respecting
        // UTF-8 character boundaries.
        let max = floor_char_boundary(remaining, max_chunk_bytes);

        // Search backwards for the last \n within the chunk.
        let last_newline = remaining[..max].rfind('\n');

        if let Some(index) = last_newline {
            // Found a newline within max_chunk_bytes. Include it in the chunk.
            callback(&remaining[..=index]);
            remaining = &remaining[index + 1..];
        } else {
            // No newline within max_chunk_bytes. Search forward for the next newline.
            let next_newline = remaining[max..].find('\n');

            if let Some(index) = next_newline {
                // Found a newline after max_chunk_bytes. Include it in the chunk.
                callback(&remaining[..=index + max]);
                remaining = &remaining[index + max + 1..];
            } else {
                // No more newlines, so print everything that's left.
                callback(remaining);
                remaining = "";
            }
        }
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

    #[test]
    fn test_print_lines_in_chunks() {
        let tests: &[(&str, &str, usize, &[&str])] = &[
            // (description, input, chunk_size, expected_chunks)
            ("empty string", "", 1024, &[]),
            ("single line no newline", "hello", 1024, &["hello"]),
            ("single line with newline", "hello\n", 1024, &["hello\n"]),
            (
                "multiple lines small",
                "line1\nline2\nline3\n",
                1024,
                &["line1\nline2\nline3\n"],
            ),
            (
                "breaks at newline",
                "line1\nline2\nline3\n",
                10,
                &["line1\n", "line2\n", "line3\n"],
            ),
            (
                "multiple lines per chunk",
                "line1\nline2\nline3\n",
                13,
                &["line1\nline2\n", "line3\n"],
            ),
            (
                "long line with newline",
                &format!("{}\n", "a".repeat(2000)),
                1024,
                &[&format!("{}\n", "a".repeat(2000))],
            ),
            (
                "long line no newline",
                &"a".repeat(2000),
                1024,
                &[&"a".repeat(2000)],
            ),
            ("exact boundary", "123456789\n", 10, &["123456789\n"]),
            (
                "newline at boundary",
                "12345678\nabcdefgh\n",
                10,
                &["12345678\n", "abcdefgh\n"],
            ),
            (
                "utf8 emoji",
                "hello😀\nworld\n",
                10,
                &["hello😀\n", "world\n"],
            ),
            (
                "utf8 near boundary",
                "1234😀\nabcd\n",
                7,
                &["1234😀\n", "abcd\n"],
            ),
            (
                "consecutive newlines",
                "line1\n\n\nline2\n",
                10,
                &["line1\n\n\n", "line2\n"],
            ),
            (
                "no trailing newline",
                "line1\nline2\nline3",
                10,
                &["line1\n", "line2\n", "line3"],
            ),
            (
                "mixed lengths",
                "short\nvery_long_line_that_exceeds_chunk_size\nmedium_line\nok\n",
                20,
                &[
                    "short\n",
                    "very_long_line_that_exceeds_chunk_size\n",
                    "medium_line\nok\n",
                ],
            ),
        ];

        for (description, input, chunk_size, expected) in tests {
            let mut chunks = Vec::new();
            print_lines_in_chunks(input, *chunk_size, |chunk| chunks.push(chunk.to_string()));
            assert_eq!(
                chunks,
                expected.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "test case: {}",
                description
            );
        }
    }
}
