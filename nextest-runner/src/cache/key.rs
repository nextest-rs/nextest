// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cache key computation.

use camino::Utf8Path;
use nextest_metadata::TestCaseName;
use std::fmt;

/// A cache key identifying a specific test result.
///
/// The key captures everything that determines whether a test should produce
/// the same result: the content of the test binary and the test name. Because
/// the binary hash changes whenever the test code is recompiled, a cached
/// result is automatically invalidated when the binary changes.
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
    pub fn test_name(&self) -> &str {
        self.test_name.as_str()
    }
}

/// A 128-bit content hash used as a compact digest of arbitrary content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContentHash {
    bytes: [u8; 16],
}

impl ContentHash {
    /// Creates a `ContentHash` from raw bytes.
    pub fn new(bytes: [u8; 16]) -> Self {
        Self { bytes }
    }

    /// Returns the hash as a lowercase hexadecimal string.
    pub fn to_hex(self) -> String {
        let mut s = String::with_capacity(32);
        for byte in &self.bytes {
            fmt::Write::write_fmt(&mut s, format_args!("{byte:02x}")).expect("writing to a String");
        }
        s
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.bytes {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Computes a [`ContentHash`] for the file at the given path.
pub fn hash_file(path: &Utf8Path) -> std::io::Result<ContentHash> {
    // TODO: this reads the whole file into memory. For large binaries we
    // should hash incrementally with a buffered reader.
    let content = std::fs::read(path)?;
    Ok(hash_bytes(&content))
}

/// Computes a [`ContentHash`] from a byte slice.
///
/// This uses XXH3, a fast non-cryptographic hash. Collision resistance is not a
/// security property here: a collision would only ever cause nextest to skip a
/// test that should have run, and the inputs (locally built test binaries) are
/// not adversarial. The 128-bit width makes accidental collisions negligible.
pub fn hash_bytes(data: &[u8]) -> ContentHash {
    let lo = xxhash_rust::xxh3::xxh3_64(data);
    let hi = xxhash_rust::xxh3::xxh3_64_with_seed(data, 1);
    let combined = (hi as u128) << 64 | lo as u128;
    ContentHash {
        bytes: combined.to_le_bytes(),
    }
}
