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
/// A result is cached only when the test cleanly passed on its first attempt.
/// Flaky (failed then passed on retry) and stress results are never cached,
/// since they are not a deterministic function of the binary — a cached failure
/// would also defeat retries.
///
/// A pass that *leaked handles* or *timed out but was tolerated* is likewise not
/// cached: those depend on subprocess and timing behavior, so caching them would
/// suppress leak and timeout detection on later runs.
pub struct CacheWriter<'a> {
    backend: &'a dyn CacheBackend,
    binary_hashes: HashMap<RustBinaryId, ContentHash>,
}

impl<'a> CacheWriter<'a> {
    /// Creates a writer that stores passing results for the given test list.
    ///
    /// Reuses the hashes computed while consulting the cache (see
    /// [`ComputedCacheInfo`]) rather than re-hashing every multi-gigabyte binary.
    /// Binaries missing from that set — which should not normally happen — are
    /// hashed as a fallback.
    ///
    /// [`ComputedCacheInfo`]: crate::cache::ComputedCacheInfo
    pub fn new(backend: &'a dyn CacheBackend, test_list: &TestList<'_>) -> Self {
        let mut binary_hashes = test_list.binary_hashes().clone();

        let missing: Vec<_> = test_list
            .iter()
            .filter(|suite| !binary_hashes.contains_key(&suite.binary_id))
            .collect();
        if !missing.is_empty() {
            // A non-empty set means the consult pass diverged from the test list.
            // Not fatal (the stragglers are hashed below), but unexpected, so
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
    /// Storage errors only warn — a failing cache never fails a passing run — but
    /// are surfaced rather than dropped, since one likely indicates a bug while
    /// this feature is experimental.
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

/// Returns true if a finished test's result may be cached: a clean
/// [`Pass`](ExecutionResultDescription::Pass), run exactly once (not retried),
/// outside a stress run. See [`CacheWriter`] for why leaky and tolerated-timeout
/// passes are excluded even though they report as success.
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
