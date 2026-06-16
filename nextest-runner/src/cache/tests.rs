// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tests for the cache module.

use crate::cache::{
    CacheEntry, CacheKey, ContentHash,
    backend::CacheBackend,
    fs_backend::FsBackend,
    imp::cache_dir_from_base,
    key::hash_bytes,
    result::{CleanPolicy, CleanStats},
};
use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use nextest_metadata::TestCaseName;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn key(binary_hash: ContentHash, test_name: &str) -> CacheKey {
    CacheKey::new(binary_hash, TestCaseName::new(test_name))
}

fn entry_at(secs: u64) -> CacheEntry {
    let time = UNIX_EPOCH + Duration::from_secs(secs);
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
    // last_hit_at is refreshed on lookup, so it is at least the stored value.
    assert!(cached.last_hit_at >= entry.last_hit_at);
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
    let old = CacheEntry {
        created_at: UNIX_EPOCH + Duration::from_secs(100),
        last_hit_at: UNIX_EPOCH + Duration::from_secs(100),
    };
    backend.store(&key(hash_bytes(b"old"), "t"), &old).unwrap();

    let now = SystemTime::now();
    let recent = CacheEntry {
        created_at: now,
        last_hit_at: now,
    };
    backend
        .store(&key(hash_bytes(b"recent"), "t"), &recent)
        .unwrap();

    let cutoff = UNIX_EPOCH + Duration::from_secs(1000);
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
fn cache_dir_prefers_xdg_cache_home() {
    let dir = cache_dir_from_base(
        Some(Utf8PathBuf::from("/tmp/xdg-cache")),
        Some(Utf8PathBuf::from("/home/someone")),
    )
    .expect("a directory should be resolved");
    assert!(
        dir.starts_with("/tmp/xdg-cache/nextest"),
        "expected XDG-rooted path, got {dir}"
    );
}

#[test]
fn cache_dir_falls_back_to_home() {
    // Both an unset and an empty XDG_CACHE_HOME should fall back to HOME.
    for xdg in [None, Some(Utf8PathBuf::new())] {
        let dir = cache_dir_from_base(xdg, Some(Utf8PathBuf::from("/home/someone")))
            .expect("a directory should be resolved");
        assert!(
            dir.starts_with("/home/someone/.cache/nextest"),
            "expected HOME/.cache-rooted path, got {dir}"
        );
    }
}

#[test]
fn cache_dir_none_without_env() {
    assert_eq!(cache_dir_from_base(None, None), None);
    assert_eq!(
        cache_dir_from_base(Some(Utf8PathBuf::new()), Some(Utf8PathBuf::new())),
        None
    );
}
