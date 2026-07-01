// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tests for the cache module.

use crate::cache::{
    CacheBinaryInput, CacheEntry, CacheKey, ComputedCacheInfo, ContentHash,
    backend::CacheBackend,
    fs_backend::FsBackend,
    imp::cache_dir_from_base,
    key::{hash_file, hash_reader},
    result::{CleanPolicy, CleanStats},
};
use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use chrono::{DateTime, Utc};
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

#[test]
fn invalidate_removes_entry() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    let k = key(hash_bytes(b"bin"), "tests::gone");

    backend.store(&k, &entry_at(1)).unwrap();
    assert!(backend.lookup(&k).unwrap().is_some());

    backend.invalidate(&k).unwrap();
    assert!(backend.lookup(&k).unwrap().is_none());
}

#[test]
fn clean_all_empties_the_cache() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));
    backend
        .store(&key(hash_bytes(b"a"), "t1"), &entry_at(1))
        .unwrap();
    backend
        .store(&key(hash_bytes(b"b"), "t2"), &entry_at(1))
        .unwrap();

    let stats = backend.clean(&CleanPolicy::All).unwrap();
    assert_eq!(stats.entries_removed, 2);

    let info = backend.info().unwrap();
    assert_eq!(info.entry_count, 0);
    assert_eq!(info.binary_count, 0);
}

#[test]
fn clean_older_than_keeps_recent_entries() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));

    // An old entry (last hit at second 100) under one binary, and a recent one
    // (last hit "now") under another.
    let old = entry_at(100);
    backend.store(&key(hash_bytes(b"old"), "t"), &old).unwrap();

    let now = Utc::now();
    let recent = CacheEntry {
        created_at: now,
        last_hit_at: now,
    };
    backend
        .store(&key(hash_bytes(b"recent"), "t"), &recent)
        .unwrap();

    let cutoff = at_secs(1000);
    let stats = backend.clean(&CleanPolicy::OlderThan(cutoff)).unwrap();
    assert_eq!(stats.entries_removed, 1);

    assert!(
        backend
            .lookup(&key(hash_bytes(b"old"), "t"))
            .unwrap()
            .is_none()
    );
    assert!(
        backend
            .lookup(&key(hash_bytes(b"recent"), "t"))
            .unwrap()
            .is_some()
    );
}

#[test]
fn clean_tolerates_a_corrupt_manifest() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("cache"));

    // A good binary alongside one whose manifest is corrupt. `clean` is a
    // management command, so a corrupt manifest must not abort the whole clean:
    // `All` still removes the directory (counting it as one entry), and
    // `OlderThan` skips it while continuing with the rest.
    backend
        .store(&key(hash_bytes(b"good"), "t"), &entry_at(1))
        .unwrap();

    let corrupt_dir = dir.path().join("cache").join("corrupt");
    std::fs::create_dir_all(&corrupt_dir).unwrap();
    std::fs::write(corrupt_dir.join("results.json"), b"not json").unwrap();

    let stats = backend
        .clean(&CleanPolicy::OlderThan(at_secs(1000)))
        .unwrap();
    // Only the good binary's single old entry is counted; the corrupt directory
    // is skipped and left in place.
    assert_eq!(stats.entries_removed, 1);
    assert!(corrupt_dir.exists());

    // `All` clears everything, including the corrupt directory (counted as one).
    let stats = backend.clean(&CleanPolicy::All).unwrap();
    assert_eq!(stats.entries_removed, 1);
    assert!(!corrupt_dir.exists());
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
fn info_on_missing_dir_is_empty() {
    let dir = Utf8TempDir::new().unwrap();
    let backend = FsBackend::new(dir.path().join("does-not-exist"));
    assert_eq!(backend.info().unwrap(), Default::default());
    assert_eq!(
        backend.clean(&CleanPolicy::All).unwrap(),
        CleanStats::default()
    );
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
