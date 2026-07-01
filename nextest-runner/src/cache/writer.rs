// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Storing passing test results in the cache.

use crate::{
    cache::{
        backend::CacheBackend,
        key::{CacheKey, ContentHash},
        result::CacheEntry,
    },
    list::TestList,
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
    binary_hashes: &'a HashMap<RustBinaryId, ContentHash>,
}

impl<'a> CacheWriter<'a> {
    /// Creates a writer that stores passing results for the given test list.
    ///
    /// Reuses the hashes computed while consulting the cache; every binary a test
    /// can run in was hashed then, so no binary is ever re-hashed here.
    pub fn new(backend: &'a dyn CacheBackend, test_list: &'a TestList<'_>) -> Self {
        Self {
            backend,
            binary_hashes: test_list.binary_hashes(),
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
