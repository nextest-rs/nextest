// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Computed cache information consulted by the test filter.

use crate::cache::{backend::CacheBackend, key::hash_file};
use camino::{Utf8Path, Utf8PathBuf};
use etcetera::{BaseStrategy, choose_base_strategy};
use iddqd::{IdOrdItem, IdOrdMap, id_upcast};
use nextest_metadata::{RustBinaryId, TestCaseName};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};
use tracing::debug;

/// The leaf path appended to the platform cache directory for result-cache
/// storage. The `v1` component lets an incompatible future layout move to a
/// fresh directory without colliding with old data.
const CACHE_SUBDIR: &[&str] = &["nextest", "result-cache", "v1"];

/// Returns the default directory for the local test result cache.
///
/// This uses the platform-native cache directory (via [`etcetera`]), matching
/// how nextest resolves other cache locations: `$XDG_CACHE_HOME` or
/// `$HOME/.cache` on Unix, `%LOCALAPPDATA%` on Windows, and the appropriate
/// directory on macOS. Returns `None` only if no base directory can be
/// determined, in which case caching is disabled rather than guessed.
pub fn default_cache_dir() -> Option<Utf8PathBuf> {
    let strategy = choose_base_strategy().ok()?;
    let base = Utf8PathBuf::from_path_buf(strategy.cache_dir()).ok()?;
    Some(cache_dir_from_base(base))
}

/// Appends the result-cache layout components to a platform cache directory.
///
/// Factored out from [`default_cache_dir`] so the layout can be tested without
/// depending on the host's environment.
pub(super) fn cache_dir_from_base(base: Utf8PathBuf) -> Utf8PathBuf {
    let mut dir = base;
    for component in CACHE_SUBDIR {
        dir.push(component);
    }
    dir
}

/// The set of tests known to be passing in the cache, keyed by binary ID.
///
/// This is computed once, before test-level filtering, by hashing each test
/// binary and querying the cache backend. The binary content hash is resolved
/// at this point, so a test name appears here only if it was cached for the
/// binary's *current* hash. As a result, [`TestFilter`] can consult this with a
/// pure name lookup — it never needs to re-hash a binary or touch the backend.
///
/// [`TestFilter`]: crate::test_filter::TestFilter
#[derive(Clone, Debug, Default)]
pub struct ComputedCacheInfo {
    /// Cached-passing tests, keyed by binary ID.
    pub test_suites: IdOrdMap<CacheTestSuiteInfo>,
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
    /// never turn a transient I/O problem into a run failure.
    ///
    /// Binaries are hashed in parallel across a small thread pool. Hashing a
    /// test binary means reading every byte of a file that is routinely several
    /// gigabytes, so the work is I/O- and CPU-bound and scales well across
    /// cores; doing it serially is the dominant cost of consulting the cache for
    /// a large workspace. The backend lookup ([`CacheBackend`] is `Send + Sync`
    /// and documents `lookup_passing` as safe to call concurrently) runs on the
    /// same worker so each binary is touched by exactly one thread.
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

        // Process each binary independently. A scoped thread pool lets the
        // worker closures borrow `work`, `backend`, and the `'a` references
        // without `'static` bounds, because the scope joins all threads before
        // returning.
        let suites = process_work(backend, &work);

        let mut test_suites = IdOrdMap::new();
        for suite in suites {
            test_suites.insert_overwrite(suite);
        }
        Self { test_suites }
    }
}

/// One binary's hashing-and-lookup unit of work, with its test names already
/// materialized into an owned set so the work is `Send`.
struct BinaryWork<'a> {
    binary_id: &'a RustBinaryId,
    binary_path: &'a Utf8Path,
    requested: BTreeSet<TestCaseName>,
}

/// Hashes and looks up every binary in `work`, returning the suites that have at
/// least one cached-passing test. Work is distributed across a bounded thread
/// pool via a shared atomic cursor (work-stealing by index), which keeps every
/// thread busy even when binaries differ wildly in size.
fn process_work(backend: &dyn CacheBackend, work: &[BinaryWork<'_>]) -> Vec<CacheTestSuiteInfo> {
    if work.is_empty() {
        return Vec::new();
    }

    // Cap the pool at the number of binaries and at the available parallelism;
    // there is no point spawning more threads than there is work or hardware.
    let parallelism = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let num_threads = parallelism.min(work.len());

    let next = AtomicUsize::new(0);
    let mut all = Vec::new();
    thread::scope(|scope| {
        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        let Some(binary) = work.get(idx) else {
                            break;
                        };
                        if let Some(suite) = consult_binary(backend, binary) {
                            local.push(suite);
                        }
                    }
                    local
                })
            })
            .collect();
        for handle in handles {
            // A worker only panics if the closure itself panics; propagate it
            // rather than silently dropping cache results.
            all.extend(handle.join().expect("cache worker thread panicked"));
        }
    });
    all
}

/// Hashes a single binary and queries the backend for its cached-passing tests.
///
/// Returns `None` (the binary's tests all run normally) on any hashing or lookup
/// error, or when nothing is cached — the cache is strictly an optimization and
/// must never turn a transient I/O problem into a run failure.
fn consult_binary(
    backend: &dyn CacheBackend,
    binary: &BinaryWork<'_>,
) -> Option<CacheTestSuiteInfo> {
    // Hash once per binary. On error, skip this binary entirely so all of its
    // tests run.
    let binary_hash = match hash_file(binary.binary_path) {
        Ok(hash) => hash,
        Err(error) => {
            debug!(
                "cache: not consulting {}: failed to hash {}: {error}",
                binary.binary_id, binary.binary_path,
            );
            return None;
        }
    };

    // Query the backend once for all of this binary's test names. A backend read
    // error degrades to "nothing cached" so every test in the binary runs
    // normally.
    let passing = match backend.lookup_passing(binary_hash, &binary.requested) {
        Ok(passing) => passing,
        Err(error) => {
            debug!(
                "cache: not consulting {}: lookup error: {error}",
                binary.binary_id,
            );
            return None;
        }
    };

    if passing.is_empty() {
        return None;
    }
    Some(CacheTestSuiteInfo {
        binary_id: binary.binary_id.clone(),
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
