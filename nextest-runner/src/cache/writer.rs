// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Storing passing test results in the cache.

use crate::{
    cache::{
        backend::{CacheBackend, CacheWrite},
        handle::CacheHandle,
        key::{CacheKey, ContentHash},
        policy::{CachePolicy, TestCacheDecision, TestExecuteResult},
    },
    config::elements::{LeakTimeoutResult, SlowTimeoutResult},
    helpers::panic_payload_to_string,
    list::{TestInstanceId, TestList},
    output_spec::OutputSpec,
    reporter::events::{
        ExecutionResultDescription, ExecutionStatuses, ReporterEvent, TestEventKind,
    },
};
use nextest_metadata::RustBinaryId;
use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
    thread::JoinHandle,
};
use tracing::warn;

/// Observes test events and writes cache updates: [`Store`](CacheWrite::Store)s
/// for clean passes this run recorded, and [`Touch`](CacheWrite::Touch)es for
/// entries it consulted.
///
/// The writer is created whenever a backend exists, regardless of policy: a run
/// that consults but does not record still touches consulted entries so eviction
/// treats them as used. Whether a finished test is stored is decided per-test by
/// the [`CachePolicy`] via [`TestCacheDecision`].
pub struct CacheWriter<'a> {
    sender: mpsc::Sender<CacheWrite>,
    handle: JoinHandle<()>,
    policy: CachePolicy,
    binary_hashes: &'a HashMap<RustBinaryId, ContentHash>,
}

impl<'a> CacheWriter<'a> {
    /// Creates a writer for `cache`, or `None` if the cache is disabled (no
    /// backend).
    ///
    /// A writer is created whenever a backend exists, regardless of policy: a run
    /// that only consults still touches consulted entries. The handle's
    /// [`CachePolicy`] then decides, per finished test, whether to record it.
    ///
    /// Reuses the hashes computed while consulting the cache; every binary a test
    /// can run in was hashed then, so no binary is ever re-hashed here.
    pub fn new(cache: CacheHandle, test_list: &'a TestList<'_>) -> Option<Self> {
        let policy = cache.policy();
        let backend = cache.backend()?;
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(move || actor(backend, receiver));
        Some(Self {
            sender,
            handle,
            policy,
            binary_hashes: test_list.binary_hashes(),
        })
    }

    /// Inspects an event and writes the corresponding cache update, if any.
    ///
    /// Storage errors only warn — a failing cache never fails a passing run — but
    /// are surfaced rather than dropped, since one likely indicates a bug while
    /// this feature is experimental.
    pub fn observe(&self, event: &ReporterEvent<'_>) {
        let ReporterEvent::Test(event) = event else {
            return;
        };

        match &event.kind {
            TestEventKind::TestFinished {
                test_instance,
                run_statuses,
                ..
            } => {
                let result = execute_result(run_statuses);
                if TestCacheDecision::compute(self.policy, result) == TestCacheDecision::RecordPass
                    && let Some(key) = self.key_from_test_instance(test_instance)
                {
                    // Ignore send errors.
                    _ = self.sender.send(CacheWrite::Store { key });
                }
            }
            TestEventKind::TestCached { test_instance, .. } => {
                let key = self
                    .key_from_test_instance(test_instance)
                    .expect("consulted test's binary was hashed, so its key is present");
                // Ignore send errors.
                _ = self.sender.send(CacheWrite::Touch { key });
            }
            _ => {}
        }
    }

    fn key_from_test_instance(&self, test_instance: &TestInstanceId<'_>) -> Option<CacheKey> {
        let binary_hash = self.binary_hashes.get(test_instance.binary_id)?;
        Some(CacheKey::new(*binary_hash, test_instance.test_name.clone()))
    }

    /// Flush the buffered writes to the cache.
    pub fn finish(self) {
        // Drop the sender, which signals the receiver to exit.
        drop(self.sender);

        // Wait for the thread to finish writing and exit.
        match self.handle.join() {
            Ok(result) => result,
            Err(panic_payload) => {
                let message = panic_payload_to_string(panic_payload);
                warn!("cache: reporter thread panicked: {message}");
            }
        }
    }
}

fn actor(backend: Arc<dyn CacheBackend>, receiver: mpsc::Receiver<CacheWrite>) {
    let mut writes = vec![];
    while let Ok(write) = receiver.recv() {
        writes.push(write);
    }

    if let Err(error) = backend.write(&writes) {
        warn!("cache: failed to write to backend: {error}");
    }
}

/// Classifies a finished test's attempts into the [`TestExecuteResult`] the
/// caching policy reasons about.
fn execute_result<S: OutputSpec>(run_statuses: &ExecutionStatuses<S>) -> TestExecuteResult {
    classify_result(&run_statuses.last_status().result, run_statuses.len() > 1)
}

/// Classifies a test's final result, given whether it was retried.
///
/// Only a single clean [`Pass`](ExecutionResultDescription::Pass) is a
/// [`CleanPass`](TestExecuteResult::CleanPass). A retried (flaky) pass, a leaky
/// pass, or a tolerated timeout is a [`TaintedPass`](TestExecuteResult::TaintedPass):
/// a success for reporting, but not a deterministic function of the binary, so
/// caching it would suppress re-detection. Everything else is a
/// [`Fail`](TestExecuteResult::Fail).
fn classify_result(result: &ExecutionResultDescription, retried: bool) -> TestExecuteResult {
    match result {
        ExecutionResultDescription::Pass if !retried => TestExecuteResult::CleanPass,
        ExecutionResultDescription::Pass
        | ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Pass,
        }
        | ExecutionResultDescription::Timeout {
            result: SlowTimeoutResult::Pass,
        } => TestExecuteResult::TaintedPass,
        _ => TestExecuteResult::Fail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reporter::events::FailureDescription;

    fn fail() -> ExecutionResultDescription {
        ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { code: 1 },
            leaked: false,
        }
    }

    #[test]
    fn clean_single_pass_is_clean() {
        assert_eq!(
            classify_result(&ExecutionResultDescription::Pass, false),
            TestExecuteResult::CleanPass,
        );
    }

    #[test]
    fn retried_pass_is_tainted() {
        // A flaky pass (passed only after a retry) is not a deterministic
        // function of the binary, so it is tainted rather than clean.
        assert_eq!(
            classify_result(&ExecutionResultDescription::Pass, true),
            TestExecuteResult::TaintedPass,
        );
    }

    #[test]
    fn leaky_and_tolerated_timeout_passes_are_tainted() {
        // Successes for reporting, but caching them would suppress leak and
        // timeout re-detection on later runs.
        assert_eq!(
            classify_result(
                &ExecutionResultDescription::Leak {
                    result: LeakTimeoutResult::Pass,
                },
                false,
            ),
            TestExecuteResult::TaintedPass,
        );
        assert_eq!(
            classify_result(
                &ExecutionResultDescription::Timeout {
                    result: SlowTimeoutResult::Pass,
                },
                false,
            ),
            TestExecuteResult::TaintedPass,
        );
    }

    #[test]
    fn failures_of_every_kind_are_fail() {
        assert_eq!(classify_result(&fail(), false), TestExecuteResult::Fail);
        assert_eq!(
            classify_result(&ExecutionResultDescription::ExecFail, false),
            TestExecuteResult::Fail,
        );
        assert_eq!(
            classify_result(
                &ExecutionResultDescription::Leak {
                    result: LeakTimeoutResult::Fail,
                },
                false,
            ),
            TestExecuteResult::Fail,
        );
        assert_eq!(
            classify_result(
                &ExecutionResultDescription::Timeout {
                    result: SlowTimeoutResult::Fail,
                },
                false,
            ),
            TestExecuteResult::Fail,
        );
    }
}
