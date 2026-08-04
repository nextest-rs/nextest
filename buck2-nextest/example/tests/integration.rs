// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tests for the `demo` library, in a second binary.
//!
//! Having two test binaries is what makes `binary_id()` filtersets meaningful
//! in this example, and this one carries the environment and artifact plumbing
//! that the unit test binary does not.

#[test]
fn add_across_crates() {
    assert_eq!(demo::add(20, 22), 42);
}

/// Reads a file Buck2 built and pointed at through the environment.
///
/// This is the interesting one: it only passes if `buck2-nextest` forwarded
/// the target's `env` from the spec, Buck2 materialized the artifact that
/// environment variable names, and the test ran in the Buck2 project root.
#[test]
fn greeting_comes_from_buck2() {
    assert_eq!(demo::greeting(), "hello from buck2");
}
