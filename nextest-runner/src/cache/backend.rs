// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The cache backend trait and error types.

use crate::cache::{
    key::{CacheKey, ContentHash},
    result::{CacheEntry, CacheInfo, CleanPolicy, CleanStats},
};
use nextest_metadata::TestCaseName;
use std::collections::BTreeSet;

/// Trait abstracting cache storage for test results.
///
/// # Contract
///
/// - **Lookup must be safe to call concurrently** from multiple threads, and is
///   read-only: it never mutates stored state.
/// - **Store is called after test execution completes,** and only for tests that passed on their
///   first attempt.
/// - **`last_hit_at` is refreshed** on store and on [`lookup_passing`](Self::lookup_passing), which
///   is the access path used before a run. Read-only [`lookup`](Self::lookup) does not refresh it.
/// - **Errors are non-fatal.** The caller treats cache failures as misses.
pub trait CacheBackend: Send + Sync {
    /// Looks up a cached result for the given key, without mutating the cache.
    ///
    /// This does not refresh `last_hit_at`; use [`lookup_passing`](Self::lookup_passing) on the
    /// pre-run path so that eviction policies see recently-consulted entries as recently used.
    fn lookup(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError>;

    /// Returns the subset of `test_names` cached as passing for `binary_hash`, refreshing the
    /// `last_hit_at` time of each match.
    ///
    /// This is the batch read path used before a run. Backends should read each binary's storage
    /// once rather than once per test name, so that consulting a binary with N cached tests costs
    /// O(1) reads rather than O(N).
    fn lookup_passing(
        &self,
        binary_hash: ContentHash,
        test_names: &BTreeSet<TestCaseName>,
    ) -> Result<BTreeSet<TestCaseName>, CacheError>;

    /// Stores a test result in the cache.
    fn store(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError>;

    /// Removes a specific entry from the cache.
    fn invalidate(&self, key: &CacheKey) -> Result<(), CacheError>;

    /// Removes cache entries according to the given policy.
    fn clean(&self, policy: &CleanPolicy) -> Result<CleanStats, CacheError>;

    /// Returns summary information about the cache.
    fn info(&self) -> Result<CacheInfo, CacheError>;
}

/// Errors that can occur during cache operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CacheError {
    /// An I/O error occurred while accessing the cache storage.
    #[error("cache I/O error")]
    Io(#[from] std::io::Error),

    /// The cached data could not be deserialized.
    #[error("cache data is corrupt or incompatible: {0}")]
    InvalidData(String),
}
