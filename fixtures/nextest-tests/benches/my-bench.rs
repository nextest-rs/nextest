// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![feature(test)]

extern crate test;

// TODO: add benchmarks
#[bench]
fn bench_add_two(b: &mut test::Bencher) {
    b.iter(|| 2 + 2);
}

#[cfg(test)]
mod tests {
    /// Test that a binary can be successfully executed.
    #[test]
    fn test_execute_bin() {
        nextest_tests::test_execute_bin_helper();
    }
}
