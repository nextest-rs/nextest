// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Computed cache information consulted by the test filter.

use crate::{
    cache::{
        backend::CacheBackend,
        key::{ContentHash, hash_file},
    },
    record::encode_workspace_path,
};
use camino::{Utf8Path, Utf8PathBuf};
use etcetera::{BaseStrategy, choose_base_strategy};
use nextest_metadata::{RustBinaryId, TestCaseName};
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};
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
/// Returns `None` if the base cache dir or canonical workspace root can't be
/// resolved; caching is then disabled rather than guessed, since it must never
/// fail a run.
pub fn default_cache_dir(workspace_root: &Utf8Path) -> Option<Utf8PathBuf> {
    let strategy = choose_base_strategy().ok()?;
    let base = Utf8PathBuf::from_path_buf(strategy.cache_dir()).ok()?;
    let canonical_workspace = workspace_root.canonicalize_utf8().ok()?;
    let encoded_workspace = encode_workspace_path(&canonical_workspace);
    Some(cache_dir_from_base(base, &encoded_workspace))
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
/// querying the backend. The hash is resolved here, so a name appears in
/// `passing` only if it was cached for the binary's *current* hash; [`TestFilter`]
/// then consults it with a pure name lookup, never re-hashing or touching the
/// backend. `binary_hashes` covers every binary that could be hashed (including
/// those with no cached passes) so the post-run [`CacheWriter`] can reuse them.
///
/// [`TestFilter`]: crate::test_filter::TestFilter
/// [`CacheWriter`]: crate::cache::CacheWriter
#[derive(Clone, Debug, Default)]
pub struct ComputedCacheInfo {
    /// Cached-passing test names, keyed by binary ID.
    pub passing: HashMap<RustBinaryId, BTreeSet<TestCaseName>>,

    /// Content hash of each binary that could be hashed, keyed by binary ID.
    pub binary_hashes: HashMap<RustBinaryId, ContentHash>,
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
        let outcomes: Vec<BinaryOutcome> = work
            .par_iter()
            .filter_map(|binary| consult_binary(backend, binary))
            .collect();

        let mut passing = HashMap::new();
        let mut binary_hashes = HashMap::with_capacity(outcomes.len());
        for outcome in outcomes {
            binary_hashes.insert(outcome.binary_id.clone(), outcome.hash);
            passing.insert(outcome.binary_id, outcome.passing);
        }
        Self {
            passing,
            binary_hashes,
        }
    }
}

/// The result of consulting one binary: its content hash, and the names of its
/// cached-passing tests (empty when nothing is cached).
struct BinaryOutcome {
    binary_id: RustBinaryId,
    hash: ContentHash,
    passing: BTreeSet<TestCaseName>,
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
fn consult_binary(backend: &dyn CacheBackend, binary: &BinaryWork<'_>) -> Option<BinaryOutcome> {
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

    // Best-effort: refresh hit times for eviction. A failure never changes what
    // is reported as cached, so it only warns.
    if !passing.is_empty()
        && let Err(error) = backend.record_access(binary_hash, &passing)
    {
        warn!(
            "cache: failed to record access for {}: {error}",
            binary.binary_id,
        );
    }

    Some(BinaryOutcome {
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
