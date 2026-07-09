// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Local filesystem cache backend.
//!
//! # Layout
//!
//! ```text
//! <cache_dir>/
//!   meta.json             # { version, last_pruned_at }
//!   <binary_hash_hex>/
//!     results.json        # { test_name -> { created_at, last_hit_at } }
//! ```

use crate::cache::{
    backend::{CacheBackend, CacheError, CacheWrite},
    key::ContentHash,
    result::PruneStats,
};
// Used only by the test-only `lookup`.
#[cfg(test)]
use crate::cache::{key::CacheKey, result::CacheEntry};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, TimeDelta, Utc};
use iddqd::{IdOrdItem, IdOrdMap, id_upcast};
use nextest_metadata::TestCaseName;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufWriter, Write},
};
use tracing::warn;

const CACHE_FORMAT_VERSION: u32 = 1;

/// The name of the top-level metadata file holding `last_pruned_at`.
const META_FILE: &str = "meta.json";

/// How long a binary's cached results are kept after it was last seen. Long
/// enough that editing a file and reverting it (which recompiles back to the old
/// binary hash) still finds the old results; beyond it, results for binaries
/// that have gone untouched are pruned.
const PRUNE_GRACE: TimeDelta = match TimeDelta::try_days(7) {
    Some(d) => d,
    None => panic!("7 days is a valid TimeDelta"),
};

/// Minimum time between automatic prunes. Mirrors the record store's cadence so
/// pruning stays off the hot path — a prune walks every cache directory, which
/// is cheap but pointless to repeat on every run.
const PRUNE_INTERVAL: TimeDelta = match TimeDelta::try_days(1) {
    Some(d) => d,
    None => panic!("1 day is a valid TimeDelta"),
};

/// A local filesystem cache backend.
pub struct FsBackend {
    cache_dir: Utf8PathBuf,
}

impl FsBackend {
    /// Creates a new filesystem backend rooted at the given directory.
    ///
    /// The directory is created lazily on first write.
    pub fn new(cache_dir: Utf8PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Returns the cache directory path.
    pub fn cache_dir(&self) -> &Utf8Path {
        &self.cache_dir
    }

    fn manifest_path(&self, binary_hash_hex: &str) -> Utf8PathBuf {
        self.cache_dir.join(binary_hash_hex).join("results.json")
    }

    fn read_manifest(&self, binary_hash_hex: &str) -> Result<Manifest, CacheError> {
        let path = self.manifest_path(binary_hash_hex);
        match fs::read(&path) {
            Ok(data) => {
                let manifest: Manifest = serde_json::from_slice(&data)
                    .map_err(|e| CacheError::InvalidData(e.to_string()))?;
                if manifest.version != CACHE_FORMAT_VERSION {
                    return Err(CacheError::InvalidData(format!(
                        "unsupported cache format version: expected {CACHE_FORMAT_VERSION}, got {}",
                        manifest.version
                    )));
                }
                Ok(manifest)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::empty()),
            Err(e) => Err(CacheError::Io(e)),
        }
    }

    fn write_manifest(&self, binary_hash_hex: &str, manifest: &Manifest) -> Result<(), CacheError> {
        let path = self.manifest_path(binary_hash_hex);
        let dir = path.parent().expect("manifest path always has a parent");
        fs::create_dir_all(dir)?;

        // Atomic (temp file + rename) so a concurrent reader or second nextest
        // process never observes a half-written manifest.
        AtomicFile::new(&path, OverwriteBehavior::AllowOverwrite)
            .write(|f| {
                let mut writer = BufWriter::new(f);
                serde_json::to_writer(&mut writer, manifest).map_err(io::Error::from)?;
                writer.flush()
            })
            .map_err(|e| match e {
                atomicwrites::Error::Internal(io) | atomicwrites::Error::User(io) => {
                    CacheError::Io(io)
                }
            })
    }

    /// Prunes stale binaries, but only if enough time has passed since the last
    /// prune. Called at the end of a run; returns `None` when the prune was
    /// skipped, `Some(stats)` when it ran.
    ///
    /// A binary is stale when its most recent access predates `now -
    /// PRUNE_GRACE`. Consulting a binary this run refreshes its access times (see
    /// [`record_access`](CacheBackend::record_access)), so anything actively used
    /// stays well clear of the cutoff. `now` is threaded in (rather than read
    /// from the clock) so tests can drive the interval and cutoff
    /// deterministically.
    ///
    /// Best-effort throughout: a failure to read or write `meta.json` degrades to
    /// "prune anyway" or "skip", never an error, since pruning must not fail a
    /// run.
    pub fn prune_if_needed(&self, now: DateTime<Utc>) -> Option<PruneStats> {
        // A missing or corrupt meta reads as "never pruned", so the first run
        // (or a run after corruption) prunes and rewrites it.
        let last_pruned_at = self.read_meta().and_then(|meta| meta.last_pruned_at);
        if let Some(last) = last_pruned_at
            && now.signed_duration_since(last) < PRUNE_INTERVAL
        {
            return None;
        }

        let stats = self.prune(now - PRUNE_GRACE);

        // Record the prune time even when nothing was removed, so the interval
        // gate holds. A write failure is non-fatal: the next run simply reads a
        // stale (or absent) time and prunes again.
        if let Err(error) = self.write_meta(&CacheMeta {
            version: CACHE_FORMAT_VERSION,
            last_pruned_at: Some(now),
        }) {
            warn!("cache: failed to update prune metadata: {error}");
        }

        Some(stats)
    }

    /// Evicts stored results for every binary whose most recent `last_hit_at` is
    /// older than `older_than`, returning what was removed. I/O errors warn and
    /// skip rather than fail a run, so this returns [`PruneStats`] directly.
    ///
    /// `pub(super)` so tests can drive the cutoff directly, bypassing the
    /// interval gate in [`prune_if_needed`](Self::prune_if_needed).
    pub(super) fn prune(&self, older_than: DateTime<Utc>) -> PruneStats {
        let mut stats = PruneStats::default();

        let read_dir = match fs::read_dir(&self.cache_dir) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return stats,
            Err(error) => {
                warn!(
                    "cache: cannot prune, failed to read {}: {error}",
                    self.cache_dir
                );
                return stats;
            }
        };

        for dir_entry in read_dir {
            let dir_entry = match dir_entry {
                Ok(entry) => entry,
                Err(error) => {
                    warn!("cache: skipping unreadable cache entry while pruning: {error}");
                    continue;
                }
            };
            let path = dir_entry.path();
            let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            // Non-hash names (meta.json, stray files) are not cache directories.
            if ContentHash::from_hex(dir_name).is_none() {
                continue;
            }

            // Leave a corrupt manifest in place: without hit times we cannot know
            // it is safe to evict.
            let manifest = match self.read_manifest(dir_name) {
                Ok(manifest) => manifest,
                Err(CacheError::InvalidData(error)) => {
                    warn!("cache: skipping {dir_name} while pruning: corrupt manifest: {error}");
                    continue;
                }
                Err(CacheError::Io(error)) => {
                    warn!("cache: skipping {dir_name} while pruning: {error}");
                    continue;
                }
            };

            // An empty manifest has no accesses, so `None` counts as stale.
            let latest_hit = manifest.entries.iter().map(|e| e.last_hit_at).max();
            if latest_hit.is_some_and(|hit| hit >= older_than) {
                continue;
            }

            let size = dir_size(&path);
            match fs::remove_dir_all(&path) {
                Ok(()) => {
                    stats.dirs_removed += 1;
                    stats.entries_removed += manifest.entries.len() as u64;
                    stats.bytes_freed += size;
                }
                Err(error) => {
                    warn!("cache: failed to remove stale cache directory {path:?}: {error}");
                }
            }
        }

        stats
    }

    /// Applies a batch of writes, stamping every entry with `now`.
    ///
    /// Takes `now` explicitly so tests can drive timestamps deterministically;
    /// the [`write`](CacheBackend::write) trait method supplies `Utc::now()`.
    pub(super) fn write_at(
        &self,
        writes: &[CacheWrite],
        now: DateTime<Utc>,
    ) -> Result<(), CacheError> {
        // Group by binary so each manifest is read, modified, and written once,
        // no matter how many of its tests appear in the batch.
        let mut by_binary: BTreeMap<String, Vec<&CacheWrite>> = BTreeMap::new();
        for write in writes {
            by_binary
                .entry(write.key().binary_hash_hex())
                .or_default()
                .push(write);
        }

        for (binary_hash_hex, binary_writes) in by_binary {
            let mut manifest = self.read_manifest(&binary_hash_hex)?;

            for write in binary_writes {
                match write {
                    // A pass: create or overwrite the entry, stamping both times.
                    CacheWrite::Store { key } => {
                        manifest.entries.insert_overwrite(ManifestEntry {
                            test_name: key.test_name().clone(),
                            created_at: now,
                            last_hit_at: now,
                        });
                    }
                    // A hit: refresh the existing entry; a missing one no-ops.
                    CacheWrite::Touch { key } => {
                        if let Some(mut entry) = manifest.entries.get_mut(key.test_name()) {
                            entry.last_hit_at = now;
                        }
                    }
                }
            }

            self.write_manifest(&binary_hash_hex, &manifest)?;
        }

        Ok(())
    }

    fn meta_path(&self) -> Utf8PathBuf {
        self.cache_dir.join(META_FILE)
    }

    /// Reads `meta.json`, returning `None` if it is missing, unreadable, or
    /// corrupt. Prune metadata is advisory, so any read problem simply behaves
    /// as "never pruned".
    fn read_meta(&self) -> Option<CacheMeta> {
        let data = fs::read(self.meta_path()).ok()?;
        let meta: CacheMeta = serde_json::from_slice(&data).ok()?;
        (meta.version == CACHE_FORMAT_VERSION).then_some(meta)
    }

    fn write_meta(&self, meta: &CacheMeta) -> Result<(), CacheError> {
        fs::create_dir_all(&self.cache_dir)?;
        AtomicFile::new(self.meta_path(), OverwriteBehavior::AllowOverwrite)
            .write(|f| {
                let mut writer = BufWriter::new(f);
                serde_json::to_writer(&mut writer, meta).map_err(io::Error::from)?;
                writer.flush()
            })
            .map_err(|e| match e {
                atomicwrites::Error::Internal(io) | atomicwrites::Error::User(io) => {
                    CacheError::Io(io)
                }
            })
    }

    /// Looks up a single cached entry, including its timestamps. Read-only.
    ///
    /// Not part of [`CacheBackend`]: the run path never inspects individual
    /// entries or their timestamps (it consults by binary via
    /// [`passing`](CacheBackend::passing)). This exists for tests to observe what
    /// a [`write`](CacheBackend::write) produced; a future single-key inspection
    /// command would lift the `cfg(test)` gate.
    #[cfg(test)]
    pub(super) fn lookup(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        let binary_hash_hex = key.binary_hash_hex();
        let manifest = self.read_manifest(&binary_hash_hex)?;

        Ok(manifest
            .entries
            .get(key.test_name())
            .map(|entry| CacheEntry {
                created_at: entry.created_at,
                last_hit_at: entry.last_hit_at,
            }))
    }
}

impl CacheBackend for FsBackend {
    fn passing(
        &self,
        binary_hash: ContentHash,
        test_names: &BTreeSet<TestCaseName>,
    ) -> Result<BTreeSet<TestCaseName>, CacheError> {
        let binary_hash_hex = binary_hash.to_hex();
        let manifest = self.read_manifest(&binary_hash_hex)?;

        // One manifest read, intersected with the requested names.
        Ok(test_names
            .iter()
            .filter(|name| manifest.entries.contains_key(name))
            .cloned()
            .collect())
    }

    fn write(&self, writes: &[CacheWrite]) -> Result<(), CacheError> {
        self.write_at(writes, Utc::now())
    }
}

/// Top-level cache metadata, stored once at the cache root in `meta.json`.
///
/// Currently just tracks the last prune so automatic pruning can be rate-limited
/// (see [`FsBackend::prune_if_needed`]).
#[derive(Debug, Serialize, Deserialize)]
struct CacheMeta {
    version: u32,
    last_pruned_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    entries: IdOrdMap<ManifestEntry>,
}

impl Manifest {
    fn empty() -> Self {
        Self {
            version: CACHE_FORMAT_VERSION,
            entries: IdOrdMap::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestEntry {
    test_name: TestCaseName,
    created_at: DateTime<Utc>,
    last_hit_at: DateTime<Utc>,
}

impl IdOrdItem for ManifestEntry {
    type Key<'a> = &'a TestCaseName;
    fn key(&self) -> Self::Key<'_> {
        &self.test_name
    }
    id_upcast!();
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut size = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                size += meta.len();
            }
        }
    }
    size
}
