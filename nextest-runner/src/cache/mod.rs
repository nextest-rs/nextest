// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test result caching.
//!
//! This module lets nextest skip re-executing tests whose result is already
//! known. A test is cached only when it passed and the test binary's content
//! is unchanged, identified by a content hash of the binary.
//!
//! - [`CacheKey`]: identifies a cached test result by binary hash and test name.
//! - [`CacheBackend`]: trait abstracting cache storage.
//! - [`FsBackend`]: local filesystem backend.
//! - [`ComputedCacheInfo`]: the precomputed set of cached-passing tests,
//!   consulted by the test filter to skip already-cached tests.
//! - [`CacheWriter`]: observes test events and stores passing results.
//!
//! The `cargo nextest cache` management CLI (clean, info) is not yet wired up,
//! so the corresponding backend methods (`clean`, `info`, `invalidate`) are
//! currently exercised only by tests.

mod backend;
mod fs_backend;
mod imp;
mod key;
mod result;
#[cfg(test)]
mod tests;
mod writer;

pub use backend::{CacheBackend, CacheError};
pub use fs_backend::FsBackend;
pub use imp::{CacheBinaryInput, CacheTestSuiteInfo, ComputedCacheInfo, default_cache_dir};
pub use key::{CacheKey, ContentHash, hash_bytes, hash_file};
pub use result::{CacheEntry, CacheInfo, CleanPolicy, CleanStats};
pub use writer::CacheWriter;
