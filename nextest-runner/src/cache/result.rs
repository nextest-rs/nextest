// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cached test result types.

use chrono::{DateTime, Utc};

/// A cached test result entry.
///
/// Only passing test results are stored in the cache. Failed and flaky tests are never cached —
/// they are always re-executed to detect intermittent issues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry {
    /// When this result was first stored in the cache.
    pub created_at: DateTime<Utc>,

    /// When this cache entry was last accessed, either stored or looked up.
    ///
    /// Drives eviction: a binary whose most recent `last_hit_at` is older than
    /// the prune cutoff (and not consulted this run) has its results removed.
    pub last_hit_at: DateTime<Utc>,
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

/// Statistics returned after a prune operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PruneStats {
    /// Number of binaries (cache directories) removed.
    pub dirs_removed: u64,

    /// Number of individual cached entries removed across those binaries.
    pub entries_removed: u64,

    /// Number of bytes freed.
    pub bytes_freed: u64,
}
