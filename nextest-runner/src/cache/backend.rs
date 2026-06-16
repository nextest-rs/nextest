// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The cache backend trait and error types.

use crate::cache::{
    key::CacheKey,
    result::{CacheEntry, CacheInfo, CleanPolicy, CleanStats},
};

/// Trait abstracting cache storage for test results.
///
/// # Contract
///
/// - **Lookup must be safe to call concurrently** from multiple threads.
/// - **Store is called after test execution completes,** and only for tests that passed on their
///   first attempt.
/// - **Backends should update `last_hit_at`** on both store and successful lookup.
/// - **Errors are non-fatal.** The caller treats cache failures as misses.
pub trait CacheBackend: Send + Sync {
    /// Looks up a cached result for the given key.
    fn lookup(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError>;

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
