// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cached test result types.

use std::time::SystemTime;

/// A cached test result entry.
///
/// Only passing test results are stored in the cache. Failed and flaky tests are never cached —
/// they are always re-executed to detect intermittent issues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry {
    /// When this result was first stored in the cache.
    pub created_at: SystemTime,

    /// When this cache entry was last accessed, either stored or looked up.
    ///
    /// Used by eviction policies (e.g., `cargo nextest cache clean --older-than 7d`).
    pub last_hit_at: SystemTime,
}

/// Summary statistics about the cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheInfo {
    /// Total number of cached entries.
    pub entry_count: u64,

    /// Total number of distinct binaries with cached results.
    pub binary_count: u64,

    /// Total size of the cache on disk, in bytes.
    pub disk_bytes: u64,
}

/// Policy for cleaning cache entries.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CleanPolicy {
    /// Remove all cache entries.
    All,

    /// Remove entries whose `last_hit_at` is older than the given time.
    OlderThan(SystemTime),
}

/// Statistics returned after a clean operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CleanStats {
    /// Number of entries removed.
    pub entries_removed: u64,

    /// Number of bytes freed.
    pub bytes_freed: u64,
}
