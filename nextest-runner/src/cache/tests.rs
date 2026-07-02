// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tests for the cache module.

use crate::cache::{
    CacheBinaryInput, CacheEntry, CacheKey, ComputedCacheInfo, ContentHash,
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

fn entry_at(secs: i64) -> CacheEntry {
    let time = at_secs(secs);
    CacheEntry {
        created_at: time,
        last_hit_at: time,
    }
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
    let entry = entry_at(1000);

    backend.store(&k, &entry).unwrap();

    let cached = backend
        .lookup(&k)
        .unwrap()
        .expect("entry should be present");
    assert_eq!(cached.created_at, entry.created_at);
    // `lookup` is read-only: it returns the stored hit time unchanged.
    assert_eq!(cached.last_hit_at, entry.last_hit_at);
}

#[test]
fn lookup_does_not_mutate_the_cache() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let bin = hash_bytes(b"bin");
    backend
        .store(&key(bin, "tests::a"), &entry_at(1000))
        .unwrap();

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
    backend.store(&key(bin, "tests::a"), &entry_at(1)).unwrap();
    backend.store(&key(bin, "tests::c"), &entry_at(1)).unwrap();

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
    backend.store(&key(bin, "tests::a"), &entry_at(1)).unwrap();

    // A pure read must leave the stored hit time untouched: refreshing is the job
    // of `record_access`, not `passing`.
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
fn record_access_refreshes_last_hit_at() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let bin = hash_bytes(b"bin");
    // Stored with an ancient hit time.
    backend.store(&key(bin, "tests::a"), &entry_at(1)).unwrap();

    backend.record_access(bin, &names(&["tests::a"])).unwrap();

    // After recording access, the hit time should be refreshed to roughly now,
    // well after the stored second-1 value.
    let refreshed = backend
        .lookup(&key(bin, "tests::a"))
        .unwrap()
        .expect("entry should be present");
    assert!(
        refreshed.last_hit_at > at_secs(1),
        "last_hit_at should be refreshed past the stored value"
    );
}

#[test]
fn different_test_names_are_independent() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let bin = hash_bytes(b"bin");

    backend.store(&key(bin, "tests::a"), &entry_at(1)).unwrap();

    assert!(backend.lookup(&key(bin, "tests::a")).unwrap().is_some());
    assert!(backend.lookup(&key(bin, "tests::b")).unwrap().is_none());
}

#[test]
fn different_binary_hashes_are_independent() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));

    // The same test name under one binary hash must not be found under another:
    // this is what makes a recompiled binary a cache miss.
    backend
        .store(&key(hash_bytes(b"old"), "tests::a"), &entry_at(1))
        .unwrap();

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
    backend.store(&key(recent, "t"), &entry_at(1500)).unwrap();
    backend.store(&key(old, "t"), &entry_at(10)).unwrap();

    let stats = backend.prune(at_secs(1000));

    assert_eq!(stats.dirs_removed, 1);
    assert_eq!(stats.entries_removed, 1);
    assert!(is_cached(&backend, recent), "recently-hit binary kept");
    assert!(!is_cached(&backend, old), "old binary evicted");
}

#[test]
fn prune_keeps_binary_refreshed_by_record_access() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));

    // A binary stored with an old hit time, then consulted this run: consulting
    // calls `record_access`, refreshing its hit time to "now".
    let consulted = hash_bytes(b"consulted");
    backend.store(&key(consulted, "t"), &entry_at(1)).unwrap();
    backend.record_access(consulted, &names(&["t"])).unwrap();

    // A prune with an old cutoff keeps it, since the refresh moved its hit time
    // (to roughly "now") well past the cutoff.
    let stats = backend.prune(at_secs(1000));
    assert_eq!(stats, Default::default());
    assert!(is_cached(&backend, consulted));
}

#[test]
fn prune_tolerates_corrupt_and_missing() {
    // A missing cache directory prunes to nothing without error.
    let dir = Utf8TempDir::new().unwrap();
    let missing = FsBackend::new(dir.path().join("does-not-exist"));
    assert_eq!(missing.prune(at_secs(1000)), Default::default());

    // A corrupt manifest is left in place rather than deleted: we cannot read its
    // hit times, so we cannot know it is safe to evict.
    let backend = FsBackend::new(dir.path().join("cache"));
    let good = hash_bytes(b"good");
    backend.store(&key(good, "t"), &entry_at(1)).unwrap();

    // Name the corrupt directory with a valid hash so it is treated as a cache
    // dir (a non-hash name would just be skipped as a stray file).
    let corrupt_hash = hash_bytes(b"corrupt").to_hex();
    let corrupt_dir = dir.path().join("cache").join(&corrupt_hash);
    std::fs::create_dir_all(&corrupt_dir).unwrap();
    std::fs::write(corrupt_dir.join("results.json"), b"not json").unwrap();

    let stats = backend.prune(at_secs(1000));
    // Only the good (stale) binary is evicted; the corrupt dir stays.
    assert_eq!(stats.dirs_removed, 1);
    assert!(!is_cached(&backend, good));
    assert!(corrupt_dir.exists());
}

#[test]
fn prune_if_needed_respects_interval() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));

    let old = hash_bytes(b"old");
    backend.store(&key(old, "t"), &entry_at(1)).unwrap();

    // First prune: no prior metadata, so it runs and evicts the stale binary.
    let now = at_secs(1_000_000);
    let first = backend.prune_if_needed(now);
    assert_eq!(first.map(|s| s.dirs_removed), Some(1));
    assert!(!is_cached(&backend, old));

    // A second call a minute later is within the 1-day interval, so it is skipped.
    let soon = now + TimeDelta::minutes(1);
    assert!(backend.prune_if_needed(soon).is_none());

    // A call more than a day later runs again (nothing to remove now).
    let later = now + TimeDelta::days(2);
    assert_eq!(backend.prune_if_needed(later), Some(Default::default()),);
}

#[test]
fn info_counts_entries_and_binaries() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let bin = hash_bytes(b"bin");
    backend.store(&key(bin, "t1"), &entry_at(1)).unwrap();
    backend.store(&key(bin, "t2"), &entry_at(1)).unwrap();
    backend
        .store(&key(hash_bytes(b"other"), "t3"), &entry_at(1))
        .unwrap();

    let info = backend.info().unwrap();
    assert_eq!(info.entry_count, 3);
    assert_eq!(info.binary_count, 2);
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
fn info_on_missing_dir_is_empty() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("does-not-exist"));
    assert_eq!(backend.info().unwrap(), Default::default());
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
    backend
        .store(&key(cached_hash, "tests::a"), &entry_at(1))
        .unwrap();

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
    assert_eq!(info.binary_hashes.get(&cached_id), Some(&cached_hash));
    assert_eq!(
        info.binary_hashes.get(&uncached_id),
        Some(&hash_file(&uncached_path).unwrap()),
    );

    // ...and only the binary with a stored pass has any passing tests.
    assert_eq!(info.passing.get(&cached_id), Some(&names(&["tests::a"])));
    assert_eq!(info.passing.get(&uncached_id), Some(&BTreeSet::new()));
}
