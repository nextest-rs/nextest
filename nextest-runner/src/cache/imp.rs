// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Computed cache information consulted by the test filter.

use crate::{
    cache::{
        backend::CacheBackend,
        key::{ContentHash, hash_file},
        parallel::parallel_filter_map,
    },
    record::encode_workspace_path,
};
use camino::{Utf8Path, Utf8PathBuf};
use etcetera::{BaseStrategy, choose_base_strategy};
use iddqd::{IdOrdItem, IdOrdMap, id_upcast};
use nextest_metadata::{RustBinaryId, TestCaseName};
use std::collections::{BTreeSet, HashMap};
use tracing::warn;

/// The leaf directory holding result-cache storage for a single workspace.
///
/// Unlike the records store, this layout carries no version component: the cache
/// format is versioned inside each manifest (see `CACHE_FORMAT_VERSION` in the
/// filesystem backend), so an incompatible future format is recognized on read
/// and treated as a miss rather than colliding with old data.
const CACHE_LEAF: &str = "result-cache";

/// Returns the default result-cache directory for the given workspace.
///
/// This mirrors how recordings resolve their state directory (see
/// [`records_state_dir`](crate::record::records_state_dir)): the cache is
/// partitioned per workspace under `nextest/projects/<encoded-workspace>/`, with
/// the workspace root canonicalized so the same workspace reached via a symlink
/// maps to the same directory.
///
/// The difference is the base: results are regenerable cache data, so this uses
/// the platform-native *cache* directory (via [`etcetera`]) — `$XDG_CACHE_HOME`
/// or `$HOME/.cache` on Unix, `%LOCALAPPDATA%` on Windows, and the appropriate
/// directory on macOS — rather than the state directory recordings use.
///
/// Returns `None` if no base cache directory can be determined or the workspace
/// root cannot be canonicalized, in which case caching is disabled rather than
/// guessed. Cache resolution is best-effort and must never fail a run.
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

/// The set of tests known to be passing in the cache, keyed by binary ID.
///
/// This is computed once, before test-level filtering, by hashing each test
/// binary and querying the cache backend. The binary content hash is resolved
/// at this point, so a test name appears here only if it was cached for the
/// binary's *current* hash. As a result, [`TestFilter`] can consult this with a
/// pure name lookup — it never needs to re-hash a binary or touch the backend.
///
/// The content hash of every binary is also retained in [`binary_hashes`]: it
/// was computed here anyway, and the post-run [`CacheWriter`] needs the same
/// hashes to store results. Carrying them forward lets the writer skip a second
/// full pass over every (multi-gigabyte) binary.
///
/// [`TestFilter`]: crate::test_filter::TestFilter
/// [`CacheWriter`]: crate::cache::CacheWriter
/// [`binary_hashes`]: Self::binary_hashes
#[derive(Clone, Debug, Default)]
pub struct ComputedCacheInfo {
    /// Cached-passing tests, keyed by binary ID.
    pub test_suites: IdOrdMap<CacheTestSuiteInfo>,

    /// The content hash of every successfully-hashed binary, keyed by binary ID.
    ///
    /// This covers all binaries that could be hashed, not just those with cached
    /// passes, so the writer can reuse it to store newly-passing results. A
    /// binary that failed to hash is absent (its results simply will not be
    /// cached).
    pub binary_hashes: HashMap<RustBinaryId, ContentHash>,
}

/// Cached-passing tests for a single test binary.
#[derive(Clone, Debug)]
pub struct CacheTestSuiteInfo {
    /// The binary ID.
    pub binary_id: RustBinaryId,

    /// The set of tests that are cached as passing for the binary's current hash.
    pub passing: BTreeSet<TestCaseName>,
}

impl IdOrdItem for CacheTestSuiteInfo {
    type Key<'a> = &'a RustBinaryId;
    fn key(&self) -> Self::Key<'_> {
        &self.binary_id
    }
    id_upcast!();
}

impl ComputedCacheInfo {
    /// Builds cache info by hashing each binary and querying the backend.
    ///
    /// `binaries` provides, for each listed test binary, its ID, the path to the
    /// compiled binary, and an iterator over the names of its test cases.
    ///
    /// Errors hashing a binary or reading the backend degrade to "not cached"
    /// (the test runs normally): the cache is strictly an optimization and must
    /// never turn a transient I/O problem into a run failure. Such errors are
    /// logged as warnings rather than dropped, so that a bug surfacing during
    /// this feature's experimental phase is visible rather than silent.
    ///
    /// Binaries are hashed in parallel across a small thread pool. Hashing a
    /// test binary means reading every byte of a file that is routinely several
    /// gigabytes, so the work is I/O- and CPU-bound and scales well across
    /// cores; doing it serially is the dominant cost of consulting the cache for
    /// a large workspace. The backend lookup ([`CacheBackend`] is `Send + Sync`
    /// and documents its reads as safe to call concurrently) runs on the same
    /// worker so each binary is touched by exactly one thread.
    pub fn collect<'a, B, N>(backend: &dyn CacheBackend, binaries: B) -> Self
    where
        B: IntoIterator<Item = CacheBinaryInput<'a, N>>,
        N: IntoIterator<Item = &'a TestCaseName>,
    {
        // Materialize the inputs up front: the per-binary test-name iterators
        // are not necessarily `Send`, so collect each into an owned set on this
        // thread before handing the work to the pool. The binary ID and path
        // references are `Send` (they borrow `Sync` data) and are kept by
        // reference to avoid cloning paths for every binary.
        let work: Vec<BinaryWork<'a>> = binaries
            .into_iter()
            .map(|binary| BinaryWork {
                binary_id: binary.binary_id,
                binary_path: binary.binary_path,
                requested: binary.test_names.into_iter().cloned().collect(),
            })
            .collect();

        // Process each binary independently across a bounded thread pool.
        let outcomes = parallel_filter_map(&work, |binary| consult_binary(backend, binary));

        let mut test_suites = IdOrdMap::new();
        let mut binary_hashes = HashMap::with_capacity(outcomes.len());
        for outcome in outcomes {
            // Retain every successful hash so the writer can reuse it, even when
            // the binary had no cached passes (`suite` is `None`).
            binary_hashes.insert(outcome.binary_id.clone(), outcome.hash);
            if let Some(suite) = outcome.suite {
                test_suites.insert_overwrite(suite);
            }
        }
        Self {
            test_suites,
            binary_hashes,
        }
    }
}

/// The result of consulting one binary: its content hash (always present, since
/// a hash failure produces no outcome at all) and the cached-passing tests, if
/// any.
struct BinaryOutcome {
    binary_id: RustBinaryId,
    hash: ContentHash,
    suite: Option<CacheTestSuiteInfo>,
}

/// One binary's hashing-and-lookup unit of work, with its test names already
/// materialized into an owned set so the work is `Send`.
struct BinaryWork<'a> {
    binary_id: &'a RustBinaryId,
    binary_path: &'a Utf8Path,
    requested: BTreeSet<TestCaseName>,
}

/// Hashes a single binary and queries the backend for its cached-passing tests.
///
/// Returns `None` only when the binary cannot be hashed — in that case its tests
/// run normally and its results are never cached. Otherwise returns the hash
/// (always, so the writer can reuse it) along with the cached-passing tests,
/// which are `None` when nothing is cached or the lookup failed. A lookup
/// failure degrades to "nothing cached" rather than failing the run.
fn consult_binary(backend: &dyn CacheBackend, binary: &BinaryWork<'_>) -> Option<BinaryOutcome> {
    // Hash once per binary. On error, skip this binary entirely so all of its
    // tests run.
    let binary_hash = hash_file(binary.binary_path)
        .inspect_err(|error| {
            warn!(
                "cache: not consulting {}: failed to hash {}: {error}",
                binary.binary_id, binary.binary_path,
            );
        })
        .ok()?;

    // Query the backend once for all of this binary's test names. A backend read
    // error degrades to "nothing cached" so every test in the binary runs
    // normally.
    let passing = backend
        .passing(binary_hash, &binary.requested)
        .inspect_err(|error| {
            warn!(
                "cache: not consulting {}: lookup error: {error}",
                binary.binary_id,
            );
        })
        .unwrap_or_default();

    // Record the consultation so eviction sees these entries as recently used.
    // This is a best-effort write: a failure never changes which tests are
    // reported as cached, so it degrades to a warning rather than discarding the
    // results computed above.
    if !passing.is_empty()
        && let Err(error) = backend.record_access(binary_hash, &passing)
    {
        warn!(
            "cache: failed to record access for {}: {error}",
            binary.binary_id,
        );
    }

    let suite = (!passing.is_empty()).then(|| CacheTestSuiteInfo {
        binary_id: binary.binary_id.clone(),
        passing,
    });
    Some(BinaryOutcome {
        binary_id: binary.binary_id.clone(),
        hash: binary_hash,
        suite,
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
