// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cache key computation.

use camino::Utf8Path;
use nextest_metadata::TestCaseName;
use std::{
    fmt,
    fs::File,
    io::{self, BufReader, Read},
};
use xxhash_rust::xxh3::Xxh3;

/// The width of a [`ContentHash`] in bytes. This and [`hash_reader`] are the
/// only places that depend on the hash algorithm.
const HASH_LEN: usize = 16;

/// A cache key identifying a specific test result: the test binary's content
/// hash and the test name. The hash changes on recompile, so a cached result is
/// automatically invalidated when the binary changes.
///
/// Tests are not sandboxed, so environment variables and other system state can
/// also affect a result; some of these may be added to the key in the future,
/// though not every case can be covered.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    binary_hash: ContentHash,
    test_name: TestCaseName,
}

impl CacheKey {
    /// Creates a cache key from a binary hash and test name.
    pub fn new(binary_hash: ContentHash, test_name: TestCaseName) -> Self {
        Self {
            binary_hash,
            test_name,
        }
    }

    /// Returns the hex-encoded binary hash component.
    pub fn binary_hash_hex(&self) -> String {
        self.binary_hash.to_hex()
    }

    /// Returns the test name component.
    pub fn test_name(&self) -> &TestCaseName {
        &self.test_name
    }
}

/// A content hash used as a compact digest of arbitrary content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContentHash {
    bytes: [u8; HASH_LEN],
}

impl ContentHash {
    /// Creates a `ContentHash` from raw bytes.
    pub fn new(bytes: [u8; HASH_LEN]) -> Self {
        Self { bytes }
    }

    /// Returns the hash as a lowercase hexadecimal string.
    pub fn to_hex(self) -> String {
        hex::encode(self.bytes)
    }

    /// Parses a hash from its hex form, the inverse of [`to_hex`].
    ///
    /// Returns `None` unless the string is exactly `2 * HASH_LEN` hex digits.
    /// Used to tell cache directories (named by hash) apart from other
    /// filesystem entries.
    ///
    /// [`to_hex`]: Self::to_hex
    pub fn from_hex(s: &str) -> Option<Self> {
        let mut bytes = [0u8; HASH_LEN];
        hex::decode_to_slice(s, &mut bytes).ok()?;
        Some(Self { bytes })
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Buffer size for streaming a file through the hasher. 256 KiB amortizes
/// syscall overhead while staying within the CPU cache.
const HASH_CHUNK_SIZE: usize = 256 * 1024;

/// Computes a [`ContentHash`] for the file at the given path, streaming it in
/// chunks so a multi-gigabyte binary hashes in constant memory.
pub fn hash_file(path: &Utf8Path) -> io::Result<ContentHash> {
    let file = File::open(path)?;
    hash_reader(BufReader::with_capacity(HASH_CHUNK_SIZE, file))
}

/// Computes a [`ContentHash`] by streaming a reader through the hasher in chunks.
///
/// Uses XXH3, a fast non-cryptographic hash. Collision resistance is not needed:
/// a collision would at worst skip a test that should have run, and locally built
/// test binaries are not adversarial. 128 bits makes accidental collisions
/// negligible.
pub fn hash_reader<R: Read>(mut reader: R) -> io::Result<ContentHash> {
    let mut hasher = Xxh3::new();
    let mut buf = [0u8; HASH_CHUNK_SIZE];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(ContentHash::new(hasher.digest128().to_le_bytes()))
}
