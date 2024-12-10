// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Builds an archive from the nextest-tests fixture, and prepares it for
//! testing.
//!
//! See the comment on `compute_dir_hash` for more information on caching and
//! invalidation.

use color_eyre::eyre::Context;
use integration_tests::seed::{
    compute_dir_hash, get_seed_archive_name, make_seed_archive, nextest_tests_dir,
};

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let tests_dir = nextest_tests_dir();
    let nextest_env_file = std::env::var("NEXTEST_ENV")
        .wrap_err("unable to find NEXTEST_ENV -- is this being run as a setup script?")?;

    // First, hash the nextest-tests fixture.
    let dir_hash = compute_dir_hash(&tests_dir)?;

    // Get the seed archive name.
    let seed_archive_name = get_seed_archive_name(dir_hash);

    // Does the seed file exist?
    if seed_archive_name.is_file() {
        println!("info: using existing seed archive: {seed_archive_name}");
    } else {
        // Otherwise, create a new seed archive.
        println!("info: unable to find seed archive {seed_archive_name}, building");
        make_seed_archive(&tests_dir, &seed_archive_name)?;
        println!("info: created new seed archive: {seed_archive_name}");
    }

    fs_err::write(
        &nextest_env_file,
        format!("SEED_ARCHIVE={seed_archive_name}"),
    )?;

    Ok(())
}
