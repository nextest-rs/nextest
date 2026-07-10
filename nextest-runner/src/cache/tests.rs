// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tests for the cache module.

use crate::cache::{
    CacheBinaryInput, CacheKey, CacheWrite, ComputedCacheInfo, ContentHash,
    backend::CacheBackend,
    fs_backend::FsBackend,
    imp::cache_dir_from_base,
    key::{hash_file, hash_reader},
};
use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use chrono::{DateTime, TimeDelta, Utc};
use nextest_metadata::{RustBinaryId, TestCaseName};
use std::collections::BTreeSet;

fn names(names: &[&str]) -> BTreeSet<TestCaseName> {
    names.iter().map(|n| TestCaseName::new(n)).collect()
}

/// Hashes an in-memory slice through the same streaming path as [`hash_file`].
///
/// A `&[u8]` is itself a `Read`, so this exercises [`hash_reader`] exactly as a
/// file would, just without touching the filesystem.
fn hash_bytes(data: &[u8]) -> ContentHash {
    hash_reader(data).expect("reading from a slice is infallible")
}

fn key(binary_hash: ContentHash, test_name: &str) -> CacheKey {
    CacheKey::new(binary_hash, TestCaseName::new(test_name))
}

fn at_secs(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).expect("timestamp is in range")
}

/// Stores a clean pass for `key`, stamped at `secs` (both `created_at` and
/// `last_hit_at`).
fn store_at(backend: &FsBackend, key: CacheKey, secs: i64) {
    backend
        .write_at(&[CacheWrite::Store { key }], at_secs(secs))
        .unwrap();
}

/// Refreshes `last_hit_at` for `test_names` under `binary_hash`, stamped at
/// `secs`. Mirrors the pre-run consult's batch of touch writes.
fn touch_at(backend: &FsBackend, binary_hash: ContentHash, test_names: &[&str], secs: i64) {
    let writes: Vec<CacheWrite> = test_names
        .iter()
        .map(|name| CacheWrite::Touch {
            key: key(binary_hash, name),
        })
        .collect();
    backend.write_at(&writes, at_secs(secs)).unwrap();
}

#[test]
fn lookup_miss_on_empty_cache() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let result = backend
        .lookup(&key(hash_bytes(b"bin"), "tests::foo"))
        .unwrap();
    assert_eq!(result, None);
}

#[test]
fn store_and_lookup_round_trips() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let k = key(hash_bytes(b"bin"), "tests::bar");

    store_at(&backend, k.clone(), 1000);

    let cached = backend
        .lookup(&k)
        .unwrap()
        .expect("entry should be present");
    // A store stamps both timestamps with the write's `now`.
    assert_eq!(cached.created_at, at_secs(1000));
    // `lookup` is read-only: it returns the stored hit time unchanged.
    assert_eq!(cached.last_hit_at, at_secs(1000));
}

#[test]
fn lookup_does_not_mutate_the_cache() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let bin = hash_bytes(b"bin");
    store_at(&backend, key(bin, "tests::a"), 1000);

    let manifest_path = dir
        .path()
        .join("cache")
        .join(bin.to_hex())
        .join("results.json");
    let before = std::fs::read(&manifest_path).unwrap();

    // The read-only methods, even on a hit, must not rewrite the manifest.
    backend.lookup(&key(bin, "tests::a")).unwrap();
    backend.passing(bin, &names(&["tests::a"])).unwrap();

    let after = std::fs::read(&manifest_path).unwrap();
    assert_eq!(before, after, "reads must not rewrite the manifest");
}

#[test]
fn passing_returns_only_cached_names() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let bin = hash_bytes(b"bin");
    store_at(&backend, key(bin, "tests::a"), 1);
    store_at(&backend, key(bin, "tests::c"), 1);

    // Request a, b, and c; only a and c are cached.
    let passing = backend
        .passing(bin, &names(&["tests::a", "tests::b", "tests::c"]))
        .unwrap();
    assert_eq!(passing, names(&["tests::a", "tests::c"]));

    // A different binary hash shares no entries.
    let passing_other = backend
        .passing(hash_bytes(b"other"), &names(&["tests::a"]))
        .unwrap();
    assert!(passing_other.is_empty());
}

#[test]
fn passing_does_not_refresh_last_hit_at() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let bin = hash_bytes(b"bin");
    store_at(&backend, key(bin, "tests::a"), 1);

    // A pure read must leave the stored hit time untouched: refreshing is the job
    // of a `Touch` write, not `passing`.
    backend.passing(bin, &names(&["tests::a"])).unwrap();

    let entry = backend
        .lookup(&key(bin, "tests::a"))
        .unwrap()
        .expect("entry should be present");
    assert_eq!(
        entry.last_hit_at,
        at_secs(1),
        "passing must not refresh last_hit_at"
    );
}

#[test]
fn touch_refreshes_last_hit_at() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let bin = hash_bytes(b"bin");
    // Stored with an ancient hit time.
    store_at(&backend, key(bin, "tests::a"), 1);

    // A touch stamped later moves the hit time forward.
    touch_at(&backend, bin, &["tests::a"], 1000);

    let refreshed = backend
        .lookup(&key(bin, "tests::a"))
        .unwrap()
        .expect("entry should be present");
    assert_eq!(
        refreshed.last_hit_at,
        at_secs(1000),
        "last_hit_at should be refreshed to the touch's timestamp"
    );
    assert_eq!(
        refreshed.created_at,
        at_secs(1),
        "a touch must not disturb created_at"
    );
}

#[test]
fn different_test_names_are_independent() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let bin = hash_bytes(b"bin");

    store_at(&backend, key(bin, "tests::a"), 1);

    assert!(backend.lookup(&key(bin, "tests::a")).unwrap().is_some());
    assert!(backend.lookup(&key(bin, "tests::b")).unwrap().is_none());
}

#[test]
fn different_binary_hashes_are_independent() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));

    // The same test name under one binary hash must not be found under another:
    // this is what makes a recompiled binary a cache miss.
    store_at(&backend, key(hash_bytes(b"old"), "tests::a"), 1);

    assert!(
        backend
            .lookup(&key(hash_bytes(b"old"), "tests::a"))
            .unwrap()
            .is_some()
    );
    assert!(
        backend
            .lookup(&key(hash_bytes(b"new"), "tests::a"))
            .unwrap()
            .is_none()
    );
}

/// Returns true if a binary's cache directory (and thus its stored results)
/// still exists on disk.
fn is_cached(backend: &FsBackend, binary_hash: ContentHash) -> bool {
    backend
        .cache_dir()
        .join(binary_hash.to_hex())
        .join("results.json")
        .exists()
}

#[test]
fn prune_evicts_stale_but_keeps_recent() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));

    // A recently-hit binary and an old one. The cutoff falls between them.
    let recent = hash_bytes(b"recent");
    let old = hash_bytes(b"old");
    store_at(&backend, key(recent, "t"), 1500);
    store_at(&backend, key(old, "t"), 10);

    let stats = backend.prune(at_secs(1000));

    assert_eq!(stats.dirs_removed, 1);
    assert_eq!(stats.entries_removed, 1);
    assert!(is_cached(&backend, recent), "recently-hit binary kept");
    assert!(!is_cached(&backend, old), "old binary evicted");
}

#[test]
fn prune_keeps_binary_refreshed_by_touch() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));

    // A binary stored with an old hit time, then consulted this run: consulting
    // issues a touch, refreshing its hit time well past the prune cutoff.
    let consulted = hash_bytes(b"consulted");
    store_at(&backend, key(consulted, "t"), 1);
    touch_at(&backend, consulted, &["t"], 2000);

    // A prune whose cutoff falls below the refreshed hit time keeps it.
    let stats = backend.prune(at_secs(1000));
    assert_eq!(stats, Default::default());
    assert!(is_cached(&backend, consulted));
}

#[test]
fn prune_missing_dir_is_empty() {
    // A missing cache directory prunes to nothing without error.
    let dir = Utf8TempDir::new().unwrap();
    let missing = FsBackend::new(dir.path().join("does-not-exist"));
    assert_eq!(missing.prune(at_secs(1000)), Default::default());
}

#[test]
fn corrupt_manifest_discards_whole_cache() {
    // A corrupt manifest self-heals by discarding the entire cache directory:
    // reading it (here via `passing`) removes the cache so the current run
    // regenerates what it needs, rather than leaving a file that can be neither
    // consulted nor refreshed.
    let dir = Utf8TempDir::new().unwrap();
    let cache_dir = dir.path().join("cache");
    let backend = FsBackend::new(cache_dir.clone());

    // A healthy binary alongside a corrupt one.
    let good = hash_bytes(b"good");
    store_at(&backend, key(good, "t"), 1);

    // Name the corrupt directory with a valid hash so it is read as a cache dir.
    let corrupt = hash_bytes(b"corrupt");
    let corrupt_dir = cache_dir.join(corrupt.to_hex());
    std::fs::create_dir_all(&corrupt_dir).unwrap();
    std::fs::write(corrupt_dir.join("results.json"), b"not json").unwrap();

    backend.passing(corrupt, &names(&["t"])).unwrap();

    // The whole cache directory is gone, healthy binary included.
    assert!(!cache_dir.exists());
    assert!(!is_cached(&backend, good));
    assert!(!is_cached(&backend, corrupt));
}

#[test]
fn prune_if_needed_respects_interval() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));

    let old = hash_bytes(b"old");
    store_at(&backend, key(old, "t"), 1);

    let grace = TimeDelta::days(7);
    let interval = TimeDelta::days(1);

    // First prune: no prior metadata, so it runs and evicts the stale binary.
    let now = at_secs(1_000_000);
    let first = backend.prune_if_needed(now, grace, interval);
    assert_eq!(first.map(|s| s.dirs_removed), Some(1));
    assert!(!is_cached(&backend, old));

    // A second call a minute later is within the interval, so it is skipped.
    let soon = now + TimeDelta::minutes(1);
    assert!(backend.prune_if_needed(soon, grace, interval).is_none());

    // A call more than an interval later runs again (nothing to remove now).
    let later = now + TimeDelta::days(2);
    assert_eq!(
        backend.prune_if_needed(later, grace, interval),
        Some(Default::default()),
    );
}

#[test]
fn hash_file_matches_hash_bytes() {
    // Streaming a file through the hasher must produce the same digest as
    // hashing the equivalent in-memory slice. Use content larger than the
    // streaming chunk size so the multi-chunk path is exercised.
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join("binary");

    let content: Vec<u8> = (0..(1024 * 1024 + 7)).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &content).unwrap();

    assert_eq!(hash_file(&path).unwrap(), hash_bytes(&content));
}

#[test]
fn hash_file_empty_matches_empty_bytes() {
    // A zero-length file must hash identically to an empty slice — the read
    // loop terminates on the first zero-length read without updating the hasher.
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join("empty");
    std::fs::write(&path, b"").unwrap();

    assert_eq!(hash_file(&path).unwrap(), hash_bytes(b""));
}

#[test]
fn hash_bytes_is_deterministic_and_content_sensitive() {
    // Same input → same hash; differing input → different hash (with
    // overwhelming probability at 128 bits).
    assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
    assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abd"));
}

#[test]
fn hash_reader_retries_on_interrupted() {
    use std::io::{self, Read};

    /// A reader that yields `Interrupted` before each real read, simulating a
    /// signal (EINTR) landing repeatedly mid-hash.
    struct InterruptingReader<'a> {
        remaining: &'a [u8],
        interrupt_next: bool,
    }

    impl Read for InterruptingReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.interrupt_next {
                self.interrupt_next = false;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            self.interrupt_next = true;
            let n = self.remaining.len().min(buf.len());
            buf[..n].copy_from_slice(&self.remaining[..n]);
            self.remaining = &self.remaining[n..];
            Ok(n)
        }
    }

    let data = b"interrupted read should not fail the hash";
    let interrupted = hash_reader(InterruptingReader {
        remaining: data,
        interrupt_next: true,
    })
    .expect("interrupted reads are retried, not propagated");

    // Retrying past the interrupts must yield the same hash as a clean read.
    assert_eq!(interrupted, hash_bytes(data));
}

#[test]
fn from_hex_round_trips_and_rejects_non_hashes() {
    // A real hash round-trips through its hex form.
    let hash = hash_bytes(b"bin");
    let hex = hash.to_hex();
    assert_eq!(hex.len(), 32, "a 16-byte hash is 32 hex digits");
    assert_eq!(ContentHash::from_hex(&hex), Some(hash));

    // Anything that is not exactly 32 hex digits is rejected: this is what lets
    // pruning tell cache directories apart from `meta.json` and stray files.
    assert_eq!(ContentHash::from_hex(""), None);
    assert_eq!(ContentHash::from_hex("meta.json"), None);
    assert_eq!(ContentHash::from_hex(&hex[..31]), None, "too short");
    assert_eq!(
        ContentHash::from_hex(&hex[..30]),
        None,
        "even but too short"
    );
    assert_eq!(ContentHash::from_hex(&format!("{hex}00")), None, "too long");
    assert_eq!(
        ContentHash::from_hex(&"g".repeat(32)),
        None,
        "right length but not hex"
    );
    // Uppercase is not produced by `to_hex`, but `decode_to_slice` accepts it;
    // that is harmless since directories are always created via `to_hex`.
    assert_eq!(ContentHash::from_hex(&hex.to_uppercase()), Some(hash));
}

#[test]
fn cache_dir_partitions_per_workspace() {
    // The layout mirrors the records store (`nextest/projects/<encoded>/`) but
    // ends in `result-cache` with no version component: the format is versioned
    // inside each manifest, not in the path.
    let dir = cache_dir_from_base(Utf8PathBuf::from("/some/cache"), "_shome_suser_sproj");
    assert_eq!(
        dir,
        "/some/cache/nextest/projects/_shome_suser_sproj/result-cache"
    );
}

#[test]
fn collect_retains_hashes_for_every_binary() {
    // The single-hash optimization relies on `collect` retaining the content
    // hash of *every* binary it could hash — including binaries with no cached
    // passes — so the writer can reuse them instead of re-hashing.
    let tmp = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(tmp.path().join("cache"));

    // Two real on-disk binaries. Only `cached` has a stored passing result; the
    // hash of `uncached` must still be retained.
    let cached_path = tmp.path().join("cached-bin");
    let uncached_path = tmp.path().join("uncached-bin");
    std::fs::write(&cached_path, b"cached binary contents").unwrap();
    std::fs::write(&uncached_path, b"uncached binary contents").unwrap();

    let cached_id = RustBinaryId::new("cached");
    let uncached_id = RustBinaryId::new("uncached");

    // Seed the cache so `cached`'s test is a hit under its real content hash.
    let cached_hash = hash_file(&cached_path).unwrap();
    store_at(&backend, key(cached_hash, "tests::a"), 1);

    let a = TestCaseName::new("tests::a");
    let info = ComputedCacheInfo::collect(
        &backend,
        vec![
            CacheBinaryInput {
                binary_id: &cached_id,
                binary_path: &cached_path,
                test_names: vec![&a],
            },
            CacheBinaryInput {
                binary_id: &uncached_id,
                binary_path: &uncached_path,
                test_names: vec![],
            },
        ],
    );

    // Both binaries' hashes are retained, matching their on-disk content...
    assert_eq!(
        info.binaries.get(&cached_id).map(|b| b.hash),
        Some(cached_hash)
    );
    assert_eq!(
        info.binaries.get(&uncached_id).map(|b| b.hash),
        Some(hash_file(&uncached_path).unwrap()),
    );

    // ...and only the binary with a stored pass has any passing tests.
    assert_eq!(
        info.binaries.get(&cached_id).map(|b| &b.passing),
        Some(&names(&["tests::a"])),
    );
    assert_eq!(
        info.binaries.get(&uncached_id).map(|b| &b.passing),
        Some(&BTreeSet::new()),
    );
}
