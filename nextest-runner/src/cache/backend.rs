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
/// Methods are split into reads ([`lookup`](Self::lookup),
/// [`passing`](Self::passing)) and writes ([`store`](Self::store),
/// [`record_access`](Self::record_access), [`invalidate`](Self::invalidate)) so
/// callers express intent rather than a read silently writing.
///
/// - Reads must be safe to call concurrently and never mutate stored state.
/// - `last_hit_at` is refreshed on store and by
///   [`record_access`](Self::record_access) so eviction sees consulted entries
///   as recently used.
/// - Errors are non-fatal but not silent: the caller treats them as misses and
///   never fails a run, but warns rather than dropping them, since one likely
///   indicates a bug while this feature is experimental.
pub trait CacheBackend: Send + Sync {
    /// Looks up a cached result for the given key. Read-only; does not refresh
    /// `last_hit_at` (that is [`record_access`](Self::record_access)'s job).
    fn lookup(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError>;

    /// Returns the subset of `test_names` cached as passing for `binary_hash`.
    /// Read-only; the pre-run caller follows it with
    /// [`record_access`](Self::record_access).
    ///
    /// Backends must read each binary's storage once, not once per name, so
    /// consulting a binary with N cached tests costs O(1) reads.
    fn passing(
        &self,
        binary_hash: ContentHash,
        test_names: &BTreeSet<TestCaseName>,
    ) -> Result<BTreeSet<TestCaseName>, CacheError>;

    /// Refreshes `last_hit_at` for each of `test_names` present under
    /// `binary_hash`, so eviction treats consulted entries as recently used.
    /// Absent names are ignored. Affects eviction ordering only, never
    /// correctness.
    fn record_access(
        &self,
        binary_hash: ContentHash,
        test_names: &BTreeSet<TestCaseName>,
    ) -> Result<(), CacheError>;

    /// Stores a test result in the cache.
    fn store(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError>;

    /// Removes a specific entry from the cache.
    fn invalidate(&self, key: &CacheKey) -> Result<(), CacheError>;

    /// Removes cache entries according to the given policy.
    ///
    /// As a management command, `clean` never runs during test execution, so an
    /// I/O error is fatal rather than treated as a miss. A corrupt manifest is
    /// tolerated so a cache whose data has gone bad can still be cleared.
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
