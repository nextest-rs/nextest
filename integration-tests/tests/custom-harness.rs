// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test that with libtest-mimic, passing in `--test-threads=1` runs the tests in a
//! single thread, and not passing it runs the tests in multiple threads.
//!
//! This is technically a fixture and should live in `fixtures/nextest-tests`,
//! but making it so pulls in several dependencies and makes the test run quite
//! a bit slower. So we make it part of integration-tests instead.
//!
//! This behavior used to be the case with libtest in the past, but was changed
//! in 2022. See <https://github.com/rust-lang/rust/issues/104053>.

use libtest_mimic::{Arguments, Trial};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = Arguments::from_args();

    let tests = vec![
        Trial::test(
            "thread_count::test_single_threaded",
            thread_count::test_single_threaded,
        )
        // Because nextest's CI runs tests against the latest stable version of
        // nextest, which doesn't have support for phase.run.extra-args yet, we
        // have to use the `with_ignored_flag` method to ignore the test. This
        // is temporary until phase.run.extra-args is in stable nextest.
        .with_ignored_flag(true),
        Trial::test(
            "thread_count::test_multi_threaded",
            thread_count::test_multi_threaded,
        ),
    ];

    libtest_mimic::run(&args, tests).exit_code()
}

// These platforms are supported by num_threads.
// https://docs.rs/num_threads/0.1.7/src/num_threads/lib.rs.html#5-8
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "ios",
    target_os = "aix"
))]
mod thread_count {
    use libtest_mimic::Failed;

    pub(crate) fn test_single_threaded() -> Result<(), Failed> {
        let num_threads = num_threads::num_threads()
            .expect("successfully obtained number of threads")
            .get();
        assert_eq!(num_threads, 1, "number of threads is 1");
        Ok(())
    }

    pub(crate) fn test_multi_threaded() -> Result<(), Failed> {
        // There must be at least two threads here, because libtest-mimic always
        // creates a second thread.
        let num_threads = num_threads::num_threads()
            .expect("successfully obtained number of threads")
            .get();
        assert!(num_threads > 1, "number of threads > 1");
        Ok(())
    }
}

// On other platforms we just say "pass" -- if/when nextest gains a way to say
// that tests were skipped at runtime, we can use that instead.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "ios",
    target_os = "aix"
)))]
mod thread_count {
    use libtest_mimic::Failed;

    pub(crate) fn test_single_threaded() -> Result<(), Failed> {
        eprintln!("skipped test on unsupported platform");
        Ok(())
    }

    pub(crate) fn test_multi_threaded() -> Result<(), Failed> {
        eprintln!("skipped test on unsupported platform");
        Ok(())
    }
}
