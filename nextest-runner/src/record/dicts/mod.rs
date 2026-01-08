// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pre-trained zstd dictionaries for compressing test output.
//!
//! These dictionaries were trained on test output from a variety of Rust
//! projects and provide ~40-60% compression improvement over standard zstd for
//! typical test stdout/stderr.
//!
//! Dictionaries are stored in each archive (`meta/stdout.dict`, `meta/stderr.dict`)
//! to make archives self-contained. This module provides the dictionaries used
//! when creating new archives.

/// Pre-trained zstd dictionary for test stdout.
///
/// Provides ~40-60% compression improvement for typical test output.
pub static STDOUT: &[u8] = include_bytes!("stdout.dict");

/// Pre-trained zstd dictionary for test stderr.
///
/// Provides ~35-60% compression improvement for typical test output.
pub static STDERR: &[u8] = include_bytes!("stderr.dict");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionaries_have_expected_sizes() {
        // These sizes are based on training with --dict-size 8192. stdout.dict
        // is exactly 8192 bytes, stderr.dict is smaller due to fewer samples.
        assert_eq!(STDOUT.len(), 8192, "stdout dictionary size");
        assert_eq!(STDERR.len(), 4809, "stderr dictionary size");
    }
}
