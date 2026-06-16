// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Storing passing test results in the cache.

use crate::{
    cache::{
        backend::CacheBackend,
        key::{CacheKey, ContentHash, hash_file},
        result::CacheEntry,
    },
    list::TestList,
    reporter::events::{ReporterEvent, TestEventKind},
};
use nextest_metadata::RustBinaryId;
use std::{collections::HashMap, time::SystemTime};
use tracing::debug;

/// Observes test events and stores passing results in the cache.
///
/// A result is cached only when the test passed on its first attempt: flaky
/// tests (failed then passed on retry) and tests run under stress are never
/// cached, because their outcome is not a deterministic function of the binary.
/// This mirrors nextest's retry model — a cached failure would defeat retries,
/// so only unambiguous passes are recorded.
///
/// Binary hashes are computed once, up front, from the test list. A binary that
/// cannot be hashed is simply never cached; this never fails the run.
pub struct CacheWriter<'a> {
    backend: &'a dyn CacheBackend,
    binary_hashes: HashMap<RustBinaryId, ContentHash>,
}

impl<'a> CacheWriter<'a> {
    /// Creates a writer that stores passing results for the given test list.
    pub fn new(backend: &'a dyn CacheBackend, test_list: &TestList<'_>) -> Self {
        let mut binary_hashes = HashMap::new();
        for suite in test_list.iter() {
            match hash_file(&suite.binary_path) {
                Ok(hash) => {
                    binary_hashes.insert(suite.binary_id.clone(), hash);
                }
                Err(error) => {
                    debug!(
                        "cache: not caching results for {}: failed to hash {}: {error}",
                        suite.binary_id, suite.binary_path,
                    );
                }
            }
        }
        Self {
            backend,
            binary_hashes,
        }
    }

    /// Inspects an event and, if it reports a clean pass, stores it in the cache.
    ///
    /// Storage errors are non-fatal: they are logged and otherwise ignored, so a
    /// failing cache never turns a passing run into a failure.
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

        // Never cache results observed under stress: the same test runs many
        // times and the outcome is intentionally not treated as a stable result.
        if stress_index.is_some() {
            return;
        }

        // Cache only a clean first-attempt pass. More than one attempt means the
        // test was retried (and is therefore flaky), and a non-successful last
        // status means it failed.
        if run_statuses.len() != 1 || !run_statuses.last_status().result.is_success() {
            return;
        }

        let Some(binary_hash) = self.binary_hashes.get(test_instance.binary_id) else {
            return;
        };

        let key = CacheKey::new(*binary_hash, test_instance.test_name.clone());
        let now = SystemTime::now();
        let entry = CacheEntry {
            created_at: now,
            last_hit_at: now,
        };
        if let Err(error) = self.backend.store(&key, &entry) {
            debug!(
                "cache: failed to store result for {} in {}: {error}",
                test_instance.test_name, test_instance.binary_id,
            );
        }
    }
}
