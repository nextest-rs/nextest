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
/// Methods are split into reads, which never mutate stored state, and writes,
/// which do. Reads ([`lookup`](Self::lookup), [`passing`](Self::passing)) and
/// writes ([`store`](Self::store), [`record_access`](Self::record_access),
/// [`invalidate`](Self::invalidate)) are separate so that callers express
/// whether they intend to mutate the cache, rather than a read silently writing.
///
/// - **Reads must be safe to call concurrently** from multiple threads, and never
///   mutate stored state.
/// - **Store is called after test execution completes,** and only for tests that passed on their
///   first attempt.
/// - **`last_hit_at` is refreshed** on store and by [`record_access`](Self::record_access), the
///   write the pre-run path issues after [`passing`](Self::passing) so eviction policies see
///   recently-consulted entries as recently used.
/// - **Errors are non-fatal but not silent.** The caller treats cache failures
///   as misses and never fails a run because of them, but surfaces them as
///   warnings rather than dropping them: while this feature is experimental, an
///   error most likely indicates a bug worth seeing.
pub trait CacheBackend: Send + Sync {
    /// Looks up a cached result for the given key. Read-only.
    ///
    /// This never refreshes `last_hit_at`; the pre-run path uses
    /// [`record_access`](Self::record_access) for that.
    fn lookup(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError>;

    /// Returns the subset of `test_names` cached as passing for `binary_hash`. Read-only.
    ///
    /// This is the batch read path used before a run; the caller follows it with
    /// [`record_access`](Self::record_access) to record the consultation. Backends should read each
    /// binary's storage once rather than once per test name, so that consulting a binary with N
    /// cached tests costs O(1) reads rather than O(N).
    fn passing(
        &self,
        binary_hash: ContentHash,
        test_names: &BTreeSet<TestCaseName>,
    ) -> Result<BTreeSet<TestCaseName>, CacheError>;

    /// Records that the given cached tests were just consulted, refreshing the `last_hit_at` time
    /// of each one present for `binary_hash`. Names with no cached entry are ignored.
    ///
    /// This is the write the pre-run path issues after [`passing`](Self::passing), so that eviction
    /// policies treat recently-consulted entries as recently used. Refreshing hit times only affects
    /// eviction ordering, never correctness.
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
