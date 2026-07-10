// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Computed cache information consulted by the test filter.

use crate::{
    cache::{
        backend::CacheBackend,
        key::{ContentHash, hash_file},
    },
    errors::CacheDirError,
    record::encode_workspace_path,
};
use camino::{Utf8Path, Utf8PathBuf};
use etcetera::{BaseStrategy, choose_base_strategy};
use iddqd::{IdHashItem, IdHashMap, id_upcast};
use nextest_metadata::{RustBinaryId, TestCaseName};
use rayon::prelude::*;
use std::collections::BTreeSet;
use tracing::warn;

/// The leaf directory holding result-cache storage for a single workspace.
///
/// No version component: the format is versioned inside each manifest (see
/// `CACHE_FORMAT_VERSION`), so an incompatible future format reads as a miss
/// rather than colliding with old data.
const CACHE_LEAF: &str = "result-cache";

/// Returns the default result-cache directory for the given workspace.
///
/// Partitioned per workspace under `nextest/projects/<encoded-workspace>/`, like
/// the records store (see [`records_state_dir`](crate::record::records_state_dir)),
/// with the root canonicalized so a symlinked workspace maps to the same dir. The
/// difference is the base: results are regenerable, so this uses the platform
/// *cache* directory (via [`etcetera`]) rather than the state directory.
///
/// Returns an error if the platform base strategy can't be determined, the cache
/// directory isn't valid UTF-8, or the workspace root can't be canonicalized. The
/// caller disables caching rather than failing the run, but surfaces the reason.
pub fn default_cache_dir(workspace_root: &Utf8Path) -> Result<Utf8PathBuf, CacheDirError> {
    let strategy = choose_base_strategy().map_err(CacheDirError::BaseDirStrategy)?;
    let cache_dir = strategy.cache_dir();
    let base = Utf8PathBuf::from_path_buf(cache_dir)
        .map_err(|path| CacheDirError::CacheDirNotUtf8 { path })?;
    let canonical_workspace =
        workspace_root
            .canonicalize_utf8()
            .map_err(|error| CacheDirError::Canonicalize {
                workspace_root: workspace_root.to_owned(),
                error,
            })?;
    let encoded_workspace = encode_workspace_path(&canonical_workspace);
    Ok(cache_dir_from_base(base, &encoded_workspace))
}

/// Builds the per-workspace result-cache directory from a base cache directory
/// and an already-encoded workspace path.
///
/// Factored out from [`default_cache_dir`] so the layout can be tested without
/// depending on the host's environment.
pub(super) fn cache_dir_from_base(base: Utf8PathBuf, encoded_workspace: &str) -> Utf8PathBuf {
    base.join("nextest")
        .join("projects")
        .join(encoded_workspace)
        .join(CACHE_LEAF)
}

/// The cached-passing tests of every consulted binary, plus each binary's hash.
///
/// Computed once, before test-level filtering, by hashing each binary and
/// querying the backend. The hash is resolved here, so a name appears in a
/// binary's `passing` set only if it was cached for that binary's *current* hash;
/// [`TestFilter`] then consults it with a pure name lookup, never re-hashing or
/// touching the backend. Every binary that could be hashed is present (including
/// those with no cached passes) so the post-run [`CacheWriter`] can reuse the
/// hashes.
///
/// [`TestFilter`]: crate::test_filter::TestFilter
/// [`CacheWriter`]: crate::cache::CacheWriter
#[derive(Clone, Debug, Default)]
pub struct ComputedCacheInfo {
    /// Per-binary cache info, keyed by binary ID.
    pub binaries: IdHashMap<BinaryCacheInfo>,
}

impl ComputedCacheInfo {
    /// Builds cache info by hashing each binary and querying the backend.
    ///
    /// Errors hashing a binary or reading the backend degrade to "not cached"
    /// (the test runs normally): the cache is an optimization and must never turn
    /// an I/O problem into a run failure. Such errors are logged as warnings, not
    /// dropped, so bugs stay visible during this feature's experimental phase.
    ///
    /// Binaries are hashed in parallel: hashing reads every byte of a
    /// routinely-multi-gigabyte file, so serial hashing dominates the cost of
    /// consulting the cache for a large workspace.
    pub fn collect<'a, B, N>(backend: &dyn CacheBackend, binaries: B) -> Self
    where
        B: IntoIterator<Item = CacheBinaryInput<'a, N>>,
        N: IntoIterator<Item = &'a TestCaseName>,
    {
        // Materialize up front: the test-name iterators are not necessarily
        // `Send`, so collect them into owned sets before handing work to the
        // pool. IDs and paths stay by reference to avoid cloning per binary.
        let work: Vec<BinaryWork<'a>> = binaries
            .into_iter()
            .map(|binary| BinaryWork {
                binary_id: binary.binary_id,
                binary_path: binary.binary_path,
                requested: binary.test_names.into_iter().cloned().collect(),
            })
            .collect();

        // Use rayon's thread pool: the work is blocking and CPU/IO-bound, and the
        // cache consult runs outside a live tokio runtime, so a thread pool fits
        // better than async here.
        //
        // Collect into a `Vec` (rayon can't build an `IdHashMap` directly), then
        // into the map. Binary IDs are unique across the work list, so no entry
        // is dropped.
        let outcomes: Vec<BinaryCacheInfo> = work
            .par_iter()
            .filter_map(|binary| consult_binary(backend, binary))
            .collect();
        Self {
            binaries: outcomes.into_iter().collect(),
        }
    }
}

/// One consulted binary's cache info: its content hash and the names of its
/// cached-passing tests (empty when nothing is cached).
#[derive(Clone, Debug)]
pub struct BinaryCacheInfo {
    /// The binary ID.
    pub binary_id: RustBinaryId,

    /// The binary's content hash, reused by the post-run writer.
    pub hash: ContentHash,

    /// The binary's cached-passing test names (empty when nothing is cached).
    pub passing: BTreeSet<TestCaseName>,
}

impl IdHashItem for BinaryCacheInfo {
    type Key<'a> = &'a RustBinaryId;
    fn key(&self) -> Self::Key<'_> {
        &self.binary_id
    }
    id_upcast!();
}

/// One binary's unit of work, with test names materialized into an owned set so
/// the work is `Send`.
struct BinaryWork<'a> {
    binary_id: &'a RustBinaryId,
    binary_path: &'a Utf8Path,
    requested: BTreeSet<TestCaseName>,
}

/// Hashes a single binary and queries the backend for its cached-passing tests.
///
/// Returns `None` only when the binary can't be hashed, so its tests run
/// normally. The hash is always returned (for the writer to reuse); `passing` is
/// empty when nothing is cached or the lookup failed.
fn consult_binary(backend: &dyn CacheBackend, binary: &BinaryWork<'_>) -> Option<BinaryCacheInfo> {
    let binary_hash = hash_file(binary.binary_path)
        .inspect_err(|error| {
            warn!(
                "cache: not consulting {}: failed to hash {}: {error}",
                binary.binary_id, binary.binary_path,
            );
        })
        .ok()?;

    // One backend read for all of this binary's names; an error degrades to
    // "nothing cached".
    let passing = backend
        .passing(binary_hash, &binary.requested)
        .inspect_err(|error| {
            warn!(
                "cache: not consulting {}: lookup error: {error}",
                binary.binary_id,
            );
        })
        .unwrap_or_default();

    Some(BinaryCacheInfo {
        binary_id: binary.binary_id.clone(),
        hash: binary_hash,
        passing,
    })
}

/// Input describing a single listed test binary, used by [`ComputedCacheInfo::collect`].
pub struct CacheBinaryInput<'a, N> {
    /// The binary ID.
    pub binary_id: &'a RustBinaryId,

    /// The path to the compiled test binary, hashed to detect changes.
    pub binary_path: &'a Utf8Path,

    /// An iterator over the names of the test cases in this binary.
    pub test_names: N,
}
