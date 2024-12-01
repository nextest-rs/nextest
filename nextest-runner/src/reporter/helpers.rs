// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use bstr::ByteSlice;

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
