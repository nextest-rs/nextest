// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A tiny library built by Buck2, used to exercise `buck2-nextest` end to end.
//!
//! There is nothing nextest-specific here. The point is to have real test
//! binaries produced by the Buck2 prelude's `rust_test` rule, so the spec
//! `buck2-nextest` consumes is the one Buck2 actually generates.

use std::{env, fs};

/// Adds two numbers.
pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

/// Returns the greeting Buck2 generated at build time.
///
/// The path comes from `DEMO_GREETING_PATH`, which `BUCK` sets to
/// `$(location :greeting)`. Buck2 writes that path relative to the project
/// root, so this only resolves if the test's working directory is the project
/// root -- see the note on working directories in `README.md`.
pub fn greeting() -> String {
    let path = env::var("DEMO_GREETING_PATH")
        .expect("DEMO_GREETING_PATH is set by the BUCK file's `env` attribute");
    let contents = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read the greeting at {path}: {error}"));
    contents.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_two_numbers() {
        assert_eq!(add(2, 2), 4);
    }

    #[test]
    fn add_is_commutative() {
        assert_eq!(add(3, 8), add(8, 3));
    }

    #[test]
    fn add_identity() {
        assert_eq!(add(41, 0), 41);
    }

    /// Ignored so that `--run-ignored` has something to demonstrate.
    #[test]
    #[ignore = "demonstrates --run-ignored"]
    fn add_saturates_at_the_maximum() {
        assert_eq!(u64::MAX.checked_add(1), None);
    }
}
