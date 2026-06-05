// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8PathBuf;

#[track_caller]
pub(crate) fn test_init() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    // The dynamic library tests require this flag.
    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe {
        std::env::set_var("RUSTFLAGS", "-C prefer-dynamic");

        // Remove OUT_DIR from the environment, as it interferes with tests
        // (some of them expect that OUT_DIR isn't set.)
        std::env::remove_var("OUT_DIR");
    }
}

pub(crate) fn fixture_project_dir() -> Utf8PathBuf {
    Utf8PathBuf::from(
        std::env::var("NEXTEST_WORKSPACE_ROOT")
            .expect("NEXTEST_WORKSPACE_ROOT is set (running under cargo nextest run)"),
    )
    .join("fixtures/fixture-project")
}
