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
    key::CacheKey,
    result::{CacheEntry, CacheInfo, CleanPolicy, CleanStats},
};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
        let data = serde_json::to_vec_pretty(manifest)
            .map_err(|e| CacheError::InvalidData(e.to_string()))?;
        fs::write(&path, &data)?;
        Ok(())
    }
}

impl CacheBackend for FsBackend {
    fn lookup(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        let binary_hash_hex = key.binary_hash_hex();
        let mut manifest = self.read_manifest(&binary_hash_hex)?;

        let Some(entry) = manifest.entries.get_mut(key.test_name()) else {
            return Ok(None);
        };

        let now = SystemTime::now();
        entry.last_hit_at = system_time_to_secs(&now);
        let result = CacheEntry {
            created_at: secs_to_system_time(entry.created_at),
            last_hit_at: now,
        };

        self.write_manifest(&binary_hash_hex, &manifest)?;

        Ok(Some(result))
    }

    fn store(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        let binary_hash_hex = key.binary_hash_hex();
        let mut manifest = self.read_manifest(&binary_hash_hex)?;

        manifest.entries.insert(
            key.test_name().to_owned(),
            ManifestEntry {
                created_at: system_time_to_secs(&entry.created_at),
                last_hit_at: system_time_to_secs(&entry.last_hit_at),
            },
        );

        self.write_manifest(&binary_hash_hex, &manifest)
    }

    fn invalidate(&self, key: &CacheKey) -> Result<(), CacheError> {
        let binary_hash_hex = key.binary_hash_hex();
        let mut manifest = self.read_manifest(&binary_hash_hex)?;

        manifest.entries.remove(key.test_name());

        if manifest.entries.is_empty() {
            let dir = self.cache_dir.join(&binary_hash_hex);
            // Removing the now-empty directory is best effort: a concurrent
            // writer may have repopulated it, and a leftover empty directory is
            // harmless.
            let _ = fs::remove_dir_all(&dir);
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

            match policy {
                CleanPolicy::All => {
                    let manifest = self.read_manifest(dir_name).ok();
                    let size = dir_size(&path);
                    fs::remove_dir_all(&path)?;
                    stats.bytes_freed += size;
                    stats.entries_removed += manifest.map(|m| m.entries.len() as u64).unwrap_or(1);
                }
                CleanPolicy::OlderThan(cutoff) => {
                    let manifest = match self.read_manifest(dir_name) {
                        Ok(m) => m,
                        Err(_) => continue,
                    };

                    let cutoff_secs = system_time_to_secs(cutoff);
                    let mut remaining = Manifest::empty();
                    for (name, entry) in manifest.entries {
                        if entry.last_hit_at < cutoff_secs {
                            stats.entries_removed += 1;
                        } else {
                            remaining.entries.insert(name, entry);
                        }
                    }

                    if remaining.entries.is_empty() {
                        let size = dir_size(&path);
                        if fs::remove_dir_all(&path).is_ok() {
                            stats.bytes_freed += size;
                        }
                    } else {
                        let _ = self.write_manifest(dir_name, &remaining);
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
    created_at: u64,
    last_hit_at: u64,
}

fn system_time_to_secs(time: &SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn secs_to_system_time(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
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
