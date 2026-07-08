// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The cache backend trait and error types.

use crate::cache::{
    key::{CacheKey, ContentHash},
    result::{CacheEntry, CacheInfo},
};
use nextest_metadata::TestCaseName;
use std::collections::BTreeSet;

/// A single write to apply to the cache.
///
/// Writes are commands, not rows: each carries only the [`CacheKey`] naming what
/// happened. Timestamps (`created_at`, `last_hit_at`) are the backend's own
/// eviction bookkeeping and are stamped by it, never supplied by the caller.
#[derive(Clone, Debug)]
pub enum CacheWrite {
    /// A test cleanly passed: create or overwrite its entry, stamping both
    /// timestamps.
    Store {
        /// The key identifying the binary and test.
        key: CacheKey,
    },

    /// A cached entry was consulted this run: refresh its `last_hit_at` so
    /// eviction treats it as recently used. A key with no stored entry is
    /// ignored.
    Touch {
        /// The key identifying the binary and test.
        key: CacheKey,
    },
}

impl CacheWrite {
    /// Returns the key this write targets.
    pub fn key(&self) -> &CacheKey {
        match self {
            CacheWrite::Store { key } | CacheWrite::Touch { key } => key,
        }
    }
}

/// Trait abstracting cache storage for test results.
///
/// This holds only storage operations (lookup, batch query, batch write, stats)
/// and assumes nothing about the underlying medium. Filesystem-specific cleanup,
/// such as directory eviction, is an inherent method on
/// [`FsBackend`](super::FsBackend) instead.
///
/// # Contract
///
/// Reads ([`lookup`](Self::lookup), [`passing`](Self::passing)) and writes
/// ([`write`](Self::write)) are separate so callers express intent rather than a
/// read silently writing.
///
/// - Reads must be safe to call concurrently and never mutate stored state.
/// - Writes are a batch of [`CacheWrite`] commands; the backend stamps
///   timestamps and groups by binary so many writes cost few storage
///   operations.
/// - Errors are non-fatal but not silent: the caller treats them as misses and
///   never fails a run, but warns rather than dropping them, since one likely
///   indicates a bug while this feature is experimental.
pub trait CacheBackend: Send + Sync {
    /// Looks up a cached result for the given key. Read-only; does not refresh
    /// `last_hit_at` (that is a [`Touch`](CacheWrite::Touch) write's job).
    fn lookup(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError>;

    /// Returns the subset of `test_names` cached as passing for `binary_hash`.
    /// Read-only; the pre-run caller follows it with a batch of
    /// [`Touch`](CacheWrite::Touch) writes.
    ///
    /// Backends must read each binary's storage once, not once per name, so
    /// consulting a binary with N cached tests costs O(1) reads.
    fn passing(
        &self,
        binary_hash: ContentHash,
        test_names: &BTreeSet<TestCaseName>,
    ) -> Result<BTreeSet<TestCaseName>, CacheError>;

    /// Applies a batch of writes. The backend stamps timestamps and groups by
    /// binary so many writes cost few storage operations.
    fn write(&self, writes: &[CacheWrite]) -> Result<(), CacheError>;

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
