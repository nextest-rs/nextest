// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Local filesystem cache backend.
//!
//! # Layout
//!
//! ```text
//! <cache_dir>/
//!   <binary_hash_hex>/
//!     results.json        # { test_name -> { created_at, last_hit_at } }
//! ```

use crate::cache::{
    backend::{CacheBackend, CacheError},
    key::{CacheKey, ContentHash},
    result::{CacheEntry, CacheInfo, CleanPolicy, CleanStats},
};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use nextest_metadata::TestCaseName;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufWriter, Write},
};
use tracing::warn;

const CACHE_FORMAT_VERSION: u32 = 1;

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
}

impl CacheBackend for FsBackend {
    fn lookup(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
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
            .filter(|name| manifest.entries.contains_key(name.as_str()))
            .cloned()
            .collect())
    }

    fn record_access(
        &self,
        binary_hash: ContentHash,
        test_names: &BTreeSet<TestCaseName>,
    ) -> Result<(), CacheError> {
        let binary_hash_hex = binary_hash.to_hex();
        let mut manifest = self.read_manifest(&binary_hash_hex)?;

        // Refresh present names in a single read-modify-write; absent ones are
        // ignored.
        let now = Utc::now();
        let mut refreshed = false;
        for name in test_names {
            if let Some(entry) = manifest.entries.get_mut(name.as_str()) {
                entry.last_hit_at = now;
                refreshed = true;
            }
        }

        if refreshed {
            self.write_manifest(&binary_hash_hex, &manifest)?;
        }

        Ok(())
    }

    fn store(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        let binary_hash_hex = key.binary_hash_hex();
        let mut manifest = self.read_manifest(&binary_hash_hex)?;

        manifest.entries.insert(
            key.test_name().to_owned(),
            ManifestEntry {
                created_at: entry.created_at,
                last_hit_at: entry.last_hit_at,
            },
        );

        self.write_manifest(&binary_hash_hex, &manifest)
    }

    fn invalidate(&self, key: &CacheKey) -> Result<(), CacheError> {
        let binary_hash_hex = key.binary_hash_hex();
        let mut manifest = self.read_manifest(&binary_hash_hex)?;

        manifest.entries.remove(key.test_name());

        if manifest.entries.is_empty() {
            // The manifest is the only file here, so drop the whole directory. A
            // failure only leaves a stale empty dir behind (no correctness
            // impact), so it warns rather than failing the call.
            let dir = self.cache_dir.join(&binary_hash_hex);
            match fs::remove_dir_all(&dir) {
                Ok(()) => {}
                // Invalidating a never-cached key reads an empty manifest and
                // lands here.
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    warn!("cache: failed to remove empty cache directory {dir}: {error}");
                }
            }
            Ok(())
        } else {
            self.write_manifest(&binary_hash_hex, &manifest)
        }
    }

    fn clean(&self, policy: &CleanPolicy) -> Result<CleanStats, CacheError> {
        let mut stats = CleanStats::default();

        let read_dir = match fs::read_dir(&self.cache_dir) {
            Ok(dir) => dir,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(stats),
            Err(e) => return Err(CacheError::Io(e)),
        };

        for dir_entry in read_dir {
            let dir_entry = dir_entry?;
            let path = dir_entry.path();
            let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            // I/O errors are fatal here; corrupt manifests stay tolerant (see
            // the trait). The two policies differ in how they handle corruption.
            match policy {
                CleanPolicy::All => {
                    // The manifest is read only to count entries removed, so a
                    // corrupt one counts as 1 rather than blocking removal.
                    let entry_count = match self.read_manifest(dir_name) {
                        Ok(manifest) => manifest.entries.len() as u64,
                        Err(CacheError::InvalidData(error)) => {
                            warn!(
                                "cache: corrupt manifest for {dir_name}, counting as 1 entry: {error}"
                            );
                            1
                        }
                        Err(error @ CacheError::Io(_)) => return Err(error),
                    };
                    let size = dir_size(&path);
                    fs::remove_dir_all(&path)?;
                    stats.bytes_freed += size;
                    stats.entries_removed += entry_count;
                }
                CleanPolicy::OlderThan(cutoff) => {
                    // A corrupt manifest has no hit times to compare, so skip
                    // that directory (with a warning) rather than fail the clean.
                    let manifest = match self.read_manifest(dir_name) {
                        Ok(manifest) => manifest,
                        Err(CacheError::InvalidData(error)) => {
                            warn!("cache: skipping {dir_name}: corrupt manifest: {error}");
                            continue;
                        }
                        Err(error @ CacheError::Io(_)) => return Err(error),
                    };

                    let mut remaining = Manifest::empty();
                    for (name, entry) in manifest.entries {
                        if entry.last_hit_at < *cutoff {
                            stats.entries_removed += 1;
                        } else {
                            remaining.entries.insert(name, entry);
                        }
                    }

                    if remaining.entries.is_empty() {
                        let size = dir_size(&path);
                        fs::remove_dir_all(&path)?;
                        stats.bytes_freed += size;
                    } else {
                        self.write_manifest(dir_name, &remaining)?;
                    }
                }
            }
        }

        Ok(stats)
    }

    fn info(&self) -> Result<CacheInfo, CacheError> {
        let mut info = CacheInfo::default();

        let read_dir = match fs::read_dir(&self.cache_dir) {
            Ok(dir) => dir,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(info),
            Err(e) => return Err(CacheError::Io(e)),
        };

        for dir_entry in read_dir {
            let dir_entry = dir_entry?;
            let path = dir_entry.path();
            let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            if let Ok(manifest) = self.read_manifest(dir_name) {
                info.binary_count += 1;
                info.entry_count += manifest.entries.len() as u64;
            }
            info.disk_bytes += dir_size(&path);
        }

        Ok(info)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    entries: BTreeMap<String, ManifestEntry>,
}

impl Manifest {
    fn empty() -> Self {
        Self {
            version: CACHE_FORMAT_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestEntry {
    created_at: DateTime<Utc>,
    last_hit_at: DateTime<Utc>,
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
