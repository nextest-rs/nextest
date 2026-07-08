// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Caching decisions, expressed independently of the cache implementation.
//!
//! This module encodes *when* nextest consults, records, or ignores the result
//! cache, as pure functions over small input structs. Keeping the decisions
//! here — rather than inline in the run path — lets them be unit-tested and
//! reasoned about on their own, and reused wherever the implementation needs
//! them.

/// The caching policy for an invocation of nextest, or for a particular test run
/// within an invocation.
///
/// This policy assumes the cache itself is enabled (e.g., `--no-cache` is not
/// present) and available (e.g., the cache directory has no permissions issues).
/// It decides whether to actually use the cache, in a read or write capacity.
#[derive(Debug, Copy, Clone, Default)]
pub struct CachePolicy {
    /// Whether cached passing results should be used if available and valid.
    pub consult: bool,
    /// Whether clean passing runs should be recorded in the cache.
    pub record: bool,
    // TODO: maybe add an evict option so tainted-pass or failing runs evict
    // cached passes?
}

impl CachePolicy {
    /// Computes the policy for a run from its [`GlobalContext`].
    pub fn compute(global_context: GlobalContext) -> Self {
        let GlobalContext {
            is_stress,
            is_with_debugger,
            is_with_tracer,
            is_bench,
            is_rerun,
        } = global_context;

        // These modes never use the cache as a *source* of passes:
        // - Stress runs exist to run each test repeatedly, so a cached pass
        //   (skipping execution) would defeat the purpose.
        // - Debugger and tracer runs target specific tests interactively;
        //   skipping them is never wanted.
        // - Benchmarks measure time, not pass/fail, so caching does not apply.
        // - Reruns only run tests that did not pass last time, so a cached pass
        //   could only wrongly skip one.
        let unusable = is_stress || is_with_debugger || is_with_tracer || is_bench;
        Self {
            consult: !unusable && !is_rerun,
            // A rerun still records passes it produces, but never consults.
            record: !unusable,
        }
    }
}

/// The aspects of a nextest run that influence its caching policy.
#[derive(Debug, Copy, Clone)]
pub struct GlobalContext {
    /// Whether this is a stress run (each test run repeatedly).
    pub is_stress: bool,
    /// Whether a debugger is attached to the run.
    pub is_with_debugger: bool,
    /// Whether a syscall tracer is attached to the run.
    pub is_with_tracer: bool,
    /// Whether this is a benchmark run.
    pub is_bench: bool,
    /// Whether this is a rerun of a previous recorded run.
    pub is_rerun: bool,
}

/// What to do with a single finished test, given the run's [`CachePolicy`] and
/// the test's outcome.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TestCacheDecision {
    /// Do not record this test result in the cache.
    Ignore,
    /// Record this test as a cached pass.
    RecordPass,
}

impl TestCacheDecision {
    /// Decides how a finished test's result should affect the cache.
    ///
    /// Only a clean pass in a recording run is stored; every other combination
    /// is ignored. Tainted passes (leaky or tolerated-timeout) and failures are
    /// never cached, since caching them would suppress re-detection.
    pub fn compute(policy: CachePolicy, result: TestExecuteResult) -> Self {
        match (policy.record, result) {
            (true, TestExecuteResult::CleanPass) => Self::RecordPass,
            (true, TestExecuteResult::TaintedPass | TestExecuteResult::Fail) => Self::Ignore,
            (false, _) => Self::Ignore,
        }
    }
}

/// The outcome of executing a test, at the granularity caching cares about.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TestExecuteResult {
    /// The test passed cleanly on its first attempt.
    CleanPass,
    /// The test passed, but in a way that should not be cached (e.g. leaked
    /// handles, a tolerated timeout, or a pass on retry).
    TaintedPass,
    /// The test failed.
    Fail,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context that permits both consulting and recording, so individual
    /// tests can flip one field at a time.
    fn permissive() -> GlobalContext {
        GlobalContext {
            is_stress: false,
            is_with_debugger: false,
            is_with_tracer: false,
            is_bench: false,
            is_rerun: false,
        }
    }

    #[test]
    fn ordinary_run_consults_and_records() {
        let policy = CachePolicy::compute(permissive());
        assert!(policy.consult);
        assert!(policy.record);
    }

    #[test]
    fn rerun_records_but_does_not_consult() {
        // A rerun only runs tests that did not pass last time, so a cached pass
        // could only wrongly skip one; but passes it does produce are worth
        // recording.
        let policy = CachePolicy::compute(GlobalContext {
            is_rerun: true,
            ..permissive()
        });
        assert!(!policy.consult);
        assert!(policy.record);
    }

    #[test]
    fn unusable_modes_neither_consult_nor_record() {
        // Each of these independently disables both directions.
        for context in [
            GlobalContext {
                is_stress: true,
                ..permissive()
            },
            GlobalContext {
                is_with_debugger: true,
                ..permissive()
            },
            GlobalContext {
                is_with_tracer: true,
                ..permissive()
            },
            GlobalContext {
                is_bench: true,
                ..permissive()
            },
        ] {
            let policy = CachePolicy::compute(context);
            assert!(!policy.consult, "should not consult: {context:?}");
            assert!(!policy.record, "should not record: {context:?}");
        }
    }

    #[test]
    fn only_clean_pass_in_recording_run_is_recorded() {
        let recording = CachePolicy {
            consult: true,
            record: true,
        };
        assert_eq!(
            TestCacheDecision::compute(recording, TestExecuteResult::CleanPass),
            TestCacheDecision::RecordPass,
        );
        // Tainted passes and failures are never recorded.
        assert_eq!(
            TestCacheDecision::compute(recording, TestExecuteResult::TaintedPass),
            TestCacheDecision::Ignore,
        );
        assert_eq!(
            TestCacheDecision::compute(recording, TestExecuteResult::Fail),
            TestCacheDecision::Ignore,
        );
    }

    #[test]
    fn non_recording_run_records_nothing() {
        let non_recording = CachePolicy {
            consult: true,
            record: false,
        };
        for result in [
            TestExecuteResult::CleanPass,
            TestExecuteResult::TaintedPass,
            TestExecuteResult::Fail,
        ] {
            assert_eq!(
                TestCacheDecision::compute(non_recording, result),
                TestCacheDecision::Ignore,
            );
        }
    }
}
