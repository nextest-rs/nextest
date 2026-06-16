// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Computed cache information consulted by the test filter.

use crate::cache::{
    backend::CacheBackend,
    key::{CacheKey, hash_file},
};
use camino::{Utf8Path, Utf8PathBuf};
use iddqd::{IdOrdItem, IdOrdMap, id_upcast};
use nextest_metadata::{RustBinaryId, TestCaseName};
use std::{collections::BTreeSet, env};
use tracing::debug;

/// Returns the default directory for the local test result cache.
///
/// This follows the XDG base directory convention: `$XDG_CACHE_HOME` if set,
/// otherwise `$HOME/.cache`, with a version component so that an incompatible
/// future layout can use a fresh directory. Returns `None` if neither variable
/// is set, in which case caching is disabled rather than guessed.
pub fn default_cache_dir() -> Option<Utf8PathBuf> {
    let xdg_cache_home =
        env::var_os("XDG_CACHE_HOME").and_then(|s| Utf8PathBuf::from_path_buf(s.into()).ok());
    let home = env::var_os("HOME").and_then(|s| Utf8PathBuf::from_path_buf(s.into()).ok());
    cache_dir_from_base(xdg_cache_home, home)
}

/// Computes the cache directory from the relevant base directories.
///
/// Factored out from [`default_cache_dir`] so the path-selection logic can be
/// tested without mutating process environment variables. Empty paths are
/// treated as unset.
pub(super) fn cache_dir_from_base(
    xdg_cache_home: Option<Utf8PathBuf>,
    home: Option<Utf8PathBuf>,
) -> Option<Utf8PathBuf> {
    let base = match xdg_cache_home {
        Some(dir) if !dir.as_str().is_empty() => dir,
        _ => {
            let home = home.filter(|h| !h.as_str().is_empty())?;
            home.join(".cache")
        }
    };
    Some(base.join("nextest").join("result-cache").join("v1"))
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
    pub fn collect<'a, B, N>(backend: &dyn CacheBackend, binaries: B) -> Self
    where
        B: IntoIterator<Item = CacheBinaryInput<'a, N>>,
        N: IntoIterator<Item = &'a TestCaseName>,
    {
        let mut test_suites = IdOrdMap::new();
        for binary in binaries {
            // Hash once per binary. On error, skip this binary entirely so all
            // of its tests run.
            let binary_hash = match hash_file(binary.binary_path) {
                Ok(hash) => hash,
                Err(error) => {
                    debug!(
                        "cache: not consulting {}: failed to hash {}: {error}",
                        binary.binary_id, binary.binary_path,
                    );
                    continue;
                }
            };

            let mut passing = BTreeSet::new();
            for test_name in binary.test_names {
                let key = CacheKey::new(binary_hash, test_name.clone());
                match backend.lookup(&key) {
                    Ok(Some(_)) => {
                        passing.insert(test_name.clone());
                    }
                    Ok(None) => {}
                    Err(error) => {
                        // Degrade to a miss: the test will run normally.
                        debug!(
                            "cache: lookup error for {} in {}: {error}",
                            test_name, binary.binary_id,
                        );
                    }
                }
            }

            if !passing.is_empty() {
                test_suites.insert_overwrite(CacheTestSuiteInfo {
                    binary_id: binary.binary_id.clone(),
                    passing,
                });
            }
        }

        Self { test_suites }
    }
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
