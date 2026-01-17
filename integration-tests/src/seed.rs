// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{env::set_env_vars, nextest_cli::CargoNextestCli};
use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::Context;
use fs_err as fs;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::SystemTime};

pub fn nextest_tests_dir() -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fixtures/nextest-tests")
}

// We use SHA-256 because other parts of nextest do the same -- this can easily
// be changed to another hash function if needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Sha256Hash([u8; 32]);

impl std::fmt::Display for Sha256Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        hex::encode(self.0).fmt(f)
    }
}

/// Computes the hash of a directory and its contents, in a way that hopefully
/// represents what Cargo does somewhat.
///
/// With any cache, invalidation is an issue -- specifically, Cargo has its own
/// notion of cache invalidation. Ideally, we could ask Cargo to give us a hash
/// for a particular command that deterministically says "a rebuild will happen
/// if and only if this hash changes". But that doesn't exist with stable Rust
/// as of this writing (Rust 1.83), so we must guess at what Cargo does.
///
/// We take some basic precautions:
///
/// * preserving mtimes while copying the source directory
/// * using both mtimes and hashes while computing the overall hash below.
///
/// Beyond that, it's possible for this implementation to have issues in three
/// different ways:
///
/// ## 1. Cargo invalidates cache, but we don't
///
/// In this case, the cache becomes useless -- Cargo will rebuild the project
/// anyway. This can cause flaky tests (see `__NEXTEST_ALT_TARGET_DIR` for a fix
/// to a flake that was caught because of this divergence).
///
/// To be clear, any divergence merely due to the cached seed not being used is
/// a bug. That was the case with the issue which `__NEXTEST_ALT_TARGET_DIR`
/// works around.
///
/// ## 2. We invalidate our cache, but Cargo doesn't
///
/// In this case, we'll regenerate a new seed but Cargo will reuse it. This
/// isn't too bad since generating the seed is a one-time cost.
///
/// ## 3. Something about the way nextest generates archives changes
///
/// This is the most difficult case to handle, because a brute hash (just hash
/// all of the files in the nextest repo) would invalidate far too often. So if
/// you're altering this code, you have to be careful to remove the cache as
/// well. Hopefully CI (which doesn't cache the seed archive) will catch issues.
///
/// ---
///
/// In general, this implementation appears to be pretty reliable, though
/// occasionally the cache has not worked (case 1 above) in Windows CI.
pub fn compute_dir_hash(dir: impl AsRef<Utf8Path>) -> color_eyre::Result<Sha256Hash> {
    let files = collect_all_files(dir.as_ref(), true)?;
    let mut hasher = Sha256::new();

    // Hash the path to `cargo` to ensure that the hash is different for
    // different Rust versions.
    hasher.update(b"nextest:cargo-path\0");
    hasher.update(
        std::env::var("CARGO")
            .expect("this should be run under cargo")
            .as_bytes(),
    );
    hasher.update([0, 0]);
    for (file_name, metadata) in files {
        hasher.update(file_name.as_str());
        hasher.update([0]);
        // Convert the system time to a number to hash.
        let timestamp = metadata
            .mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("file's mtime after 1970-01-01");
        hasher.update(timestamp.as_nanos().to_le_bytes());
        hasher.update(metadata.hash.0);
        hasher.update([0]);
    }
    Ok(Sha256Hash(hasher.finalize().into()))
}

// Hash and collect metadata about all the files in a directory.
//
// Using a `BTreeMap` ensures a deterministic order of files above.
fn collect_all_files(
    dir: &Utf8Path,
    root: bool,
) -> color_eyre::Result<BTreeMap<Utf8PathBuf, FileMetadata>> {
    let mut stack = vec![dir.to_path_buf()];
    let mut hashes = BTreeMap::new();

    // TODO: parallelize this?
    while let Some(dir) = stack.pop() {
        for entry in dir.read_dir_utf8()? {
            let entry =
                entry.wrap_err_with(|| format!("failed to read entry from directory {dir}"))?;
            let ty = entry
                .file_type()
                .wrap_err_with(|| format!("failed to get file type for entry {}", entry.path()))?;

            // Ignore a pre-existing `target` directory at the root.
            if root && entry.path().file_name() == Some("target") {
                continue;
            }

            if ty.is_dir() {
                stack.push(entry.into_path());
            } else if ty.is_file() {
                let metadata = entry.metadata().wrap_err_with(|| {
                    format!("failed to get metadata for file {}", entry.path())
                })?;

                // Also include the mtime, because Cargo uses the mtime to
                // determine if a local file has changed. If there were a way to
                // tell Cargo to ignore mtimes, we could remove this.
                let mtime = metadata.modified().wrap_err_with(|| {
                    format!("failed to get modified time for file {}", entry.path())
                })?;
                let path = entry.into_path();
                let contents = fs::read(&path)?;
                let hash = Sha256Hash(Sha256::digest(&contents).into());
                hashes.insert(path, FileMetadata { mtime, hash });
            }
        }
    }

    Ok(hashes)
}

#[derive(Clone, Debug)]
struct FileMetadata {
    mtime: SystemTime,
    hash: Sha256Hash,
}

pub fn get_seed_archive_name(hash: Sha256Hash) -> Utf8PathBuf {
    // Check in the std temp directory for the seed file.
    let temp_dir = Utf8PathBuf::try_from(std::env::temp_dir()).expect("temp dir is utf-8");
    let username = whoami::username().expect("obtained username");
    let user_dir = temp_dir.join(format!("nextest-tests-seed-{username}"));
    user_dir.join(format!("seed-{hash}.tar.zst"))
}

pub fn make_seed_archive(workspace_dir: &Utf8Path, file_name: &Utf8Path) -> color_eyre::Result<()> {
    // Make the directory containing the file name.
    fs::create_dir_all(file_name.parent().unwrap())?;

    // First, run a build in a temporary directory.
    let temp_dir = camino_tempfile::Builder::new()
        .prefix("nextest-seed-build-")
        .tempdir()
        .wrap_err("failed to create temporary directory")?;
    let target_dir = temp_dir.path().join("target");
    fs::create_dir_all(&target_dir)?;

    // Now build a nextest archive, using the temporary directory as the target dir.
    let mut cli = CargoNextestCli::for_script()?;

    // Set the environment variables after getting the CLI -- this avoids
    // rebuilds due to the variables changing.
    //
    // TODO: We shouldn't alter the global state of this process -- instead,
    // set_env_vars should be part of nextest_cli.rs.
    set_env_vars();

    let output = cli
        .args([
            "--manifest-path",
            workspace_dir.join("Cargo.toml").as_str(),
            "archive",
            "--archive-file",
            file_name.as_str(),
            "--workspace",
            "--all-targets",
            "--target-dir",
            target_dir.as_str(),
            // Use this profile to ensure that the entire target dir is included.
            "--profile",
            "archive-all",
        ])
        .output();

    if std::env::var("INTEGRATION_TESTS_DEBUG") == Ok("1".to_string()) {
        eprintln!("make_seed_archive output: {output}");
    }

    Ok(())
}
