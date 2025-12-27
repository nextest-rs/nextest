// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![feature(test)]

extern crate test;

fn check_run_mode() {
    let has_bench_flag = std::env::args().any(|arg| arg == "--bench");
    let run_mode = std::env::var("NEXTEST_RUN_MODE").expect("NEXTEST_RUN_MODE must be set");

    if has_bench_flag {
        assert_eq!(
            run_mode, "benchmark",
            "NEXTEST_RUN_MODE should be 'benchmark' when --bench is passed"
        );
    } else {
        assert_eq!(
            run_mode, "test",
            "NEXTEST_RUN_MODE should be 'test' when --bench is not passed"
        );
    }
}

#[bench]
fn bench_add_two(b: &mut test::Bencher) {
    check_run_mode();
    b.iter(|| 2 + 2);
}

#[bench]
#[ignore]
fn bench_ignored(b: &mut test::Bencher) {
    check_run_mode();
    b.iter(|| 2 + 2);
}

#[bench]
#[ignore]
fn bench_slow_timeout(_b: &mut test::Bencher) {
    check_run_mode();
    // Sleep long enough to trigger timeout when bench.slow-timeout uses
    // period = "500ms", terminate-after = 2 (1 second total).
    // But not long enough to trigger when using default bench.slow-timeout (30y).
    std::thread::sleep(std::time::Duration::from_millis(1500));
}

#[cfg(test)]
mod tests {
    /// Test that a binary can be successfully executed.
    #[test]
    fn test_execute_bin() {
        nextest_tests::test_execute_bin_helper();
    }
}
