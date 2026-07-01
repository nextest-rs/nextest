// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Storing passing test results in the cache.

use crate::{
    cache::{
        backend::CacheBackend,
        key::{CacheKey, ContentHash, hash_file},
        parallel::parallel_filter_map,
        result::CacheEntry,
    },
    list::{RustTestSuite, TestList},
    reporter::events::{ExecutionResultDescription, ReporterEvent, TestEventKind},
};
use chrono::Utc;
use nextest_metadata::RustBinaryId;
use std::collections::HashMap;
use tracing::warn;

/// Observes test events and stores passing results in the cache.
///
/// A result is cached only when the test cleanly passed on its first attempt:
/// flaky tests (failed then passed on retry) and tests run under stress are
/// never cached, because their outcome is not a deterministic function of the
/// binary. This mirrors nextest's retry model — a cached failure would defeat
/// retries, so only unambiguous passes are recorded.
///
/// A pass that also *leaked handles* or *timed out but was tolerated* is
/// likewise not cached: those outcomes depend on subprocess and timing behavior
/// rather than binary content, so caching them would silently suppress leak and
/// timeout detection on subsequent runs.
///
/// Binary hashes are computed once, up front, from the test list. A binary that
/// cannot be hashed is simply never cached; this never fails the run.
pub struct CacheWriter<'a> {
    backend: &'a dyn CacheBackend,
    binary_hashes: HashMap<RustBinaryId, ContentHash>,
}

impl<'a> CacheWriter<'a> {
    /// Creates a writer that stores passing results for the given test list.
    ///
    /// The content hashes were already computed while consulting the cache
    /// before the run (see [`ComputedCacheInfo`]), so they are reused here rather
    /// than recomputed: hashing reads every byte of each (multi-gigabyte) binary,
    /// and doing it a second time would roughly double the cache's overhead. Only
    /// binaries missing from the precomputed set — which should not normally
    /// happen — are hashed as a fallback, in parallel.
    ///
    /// [`ComputedCacheInfo`]: crate::cache::ComputedCacheInfo
    pub fn new(backend: &'a dyn CacheBackend, test_list: &TestList<'_>) -> Self {
        let mut binary_hashes = test_list.binary_hashes().clone();

        // Fallback: hash any suite whose hash was not carried over from the
        // consult pass. In practice the consult pass hashes every binary, so
        // this is empty; it exists so the writer is correct even if the two
        // sets ever diverge.
        let missing: Vec<_> = test_list
            .iter()
            .filter(|suite| !binary_hashes.contains_key(&suite.binary_id))
            .collect();
        if !missing.is_empty() {
            // The consult pass is expected to hash every binary, so a non-empty
            // set here means the two passes diverged. That is not fatal — the
            // fallback below hashes the stragglers — but it is unexpected, so
            // surface it while the feature is experimental.
            warn!(
                "cache: {} binaries were not covered by the consult pass; hashing them now",
                missing.len(),
            );
            binary_hashes.extend(hash_binaries(&missing));
        }

        Self {
            backend,
            binary_hashes,
        }
    }

    /// Inspects an event and, if it reports a clean pass, stores it in the cache.
    ///
    /// Storage errors do not fail the run — a failing cache never turns a passing
    /// run into a failure — but they are surfaced as warnings rather than silently
    /// dropped: while this feature is experimental, a store failure most likely
    /// indicates a bug we want to see, not a benign condition.
    pub fn observe(&self, event: &ReporterEvent<'_>) {
        let ReporterEvent::Test(event) = event else {
            return;
        };
        let TestEventKind::TestFinished {
            stress_index,
            test_instance,
            run_statuses,
            ..
        } = &event.kind
        else {
            return;
        };

        if !is_cacheable(
            stress_index.is_some(),
            run_statuses.len(),
            &run_statuses.last_status().result,
        ) {
            return;
        }

        let Some(binary_hash) = self.binary_hashes.get(test_instance.binary_id) else {
            return;
        };

        let key = CacheKey::new(*binary_hash, test_instance.test_name.clone());
        let now = Utc::now();
        let entry = CacheEntry {
            created_at: now,
            last_hit_at: now,
        };
        if let Err(error) = self.backend.store(&key, &entry) {
            warn!(
                "cache: failed to store result for {} in {}: {error}",
                test_instance.test_name, test_instance.binary_id,
            );
        }
    }
}

/// Hashes every test suite's binary in parallel, returning a map from binary ID
/// to content hash. A binary that cannot be hashed is simply omitted: its
/// results will never be cached, which never fails the run.
fn hash_binaries(suites: &[&RustTestSuite<'_>]) -> HashMap<RustBinaryId, ContentHash> {
    parallel_filter_map(suites, |suite| {
        hash_file(&suite.binary_path)
            .inspect_err(|error| {
                warn!(
                    "cache: not caching results for {}: failed to hash {}: {error}",
                    suite.binary_id, suite.binary_path,
                );
            })
            .ok()
            .map(|hash| (suite.binary_id.clone(), hash))
    })
    .into_iter()
    .collect()
}

/// Returns true if a finished test's result may be cached.
///
/// A result is cacheable only when all of the following hold:
///
/// - It was not observed under stress (`under_stress` is false): a stress run
///   executes the same test many times and its outcome is not a stable result.
/// - It ran exactly once (`attempt_count == 1`): more than one attempt means the
///   test was retried and is therefore flaky.
/// - Its result is a clean [`Pass`](ExecutionResultDescription::Pass). We
///   deliberately do not accept every `is_success()` outcome: a leaky or
///   timeout-but-tolerated pass counts as success for reporting, but its outcome
///   depends on subprocess and timing behavior rather than binary content, so
///   caching it would suppress leak and timeout detection on later runs.
fn is_cacheable(
    under_stress: bool,
    attempt_count: usize,
    result: &ExecutionResultDescription,
) -> bool {
    !under_stress && attempt_count == 1 && matches!(result, ExecutionResultDescription::Pass)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::elements::{LeakTimeoutResult, SlowTimeoutResult},
        reporter::events::FailureDescription,
    };

    fn fail() -> ExecutionResultDescription {
        ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { code: 1 },
            leaked: false,
        }
    }

    #[test]
    fn only_clean_single_pass_is_cacheable() {
        // The one cacheable case: a clean, single-attempt, non-stress pass.
        assert!(is_cacheable(false, 1, &ExecutionResultDescription::Pass));

        // Stress runs are never cached, even on a clean pass.
        assert!(!is_cacheable(true, 1, &ExecutionResultDescription::Pass));

        // Retried (flaky) passes are never cached.
        assert!(!is_cacheable(false, 2, &ExecutionResultDescription::Pass));
        assert!(!is_cacheable(false, 3, &ExecutionResultDescription::Pass));

        // A leaky pass is a success for reporting but is not cached: the leak
        // must be re-detected on the next run.
        assert!(!is_cacheable(
            false,
            1,
            &ExecutionResultDescription::Leak {
                result: LeakTimeoutResult::Pass,
            },
        ));

        // A tolerated timeout (treated as a pass) is likewise not cached.
        assert!(!is_cacheable(
            false,
            1,
            &ExecutionResultDescription::Timeout {
                result: SlowTimeoutResult::Pass,
            },
        ));

        // Failures of every kind are not cached.
        assert!(!is_cacheable(false, 1, &fail()));
        assert!(!is_cacheable(
            false,
            1,
            &ExecutionResultDescription::ExecFail
        ));
        assert!(!is_cacheable(
            false,
            1,
            &ExecutionResultDescription::Leak {
                result: LeakTimeoutResult::Fail,
            },
        ));
    }
}
