// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::{Utf8Path, Utf8PathBuf};
use std::env;

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

        std::env::set_var(
            "__NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_NO_OVERRIDE",
            "test-PASSED-value-set-by-environment",
        );
        std::env::set_var(
            "__NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_OVERRIDDEN",
            "test-FAILED-value-set-by-environment",
        );
        std::env::set_var(
            "__NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_RELATIVE_NO_OVERRIDE",
            "test-PASSED-value-set-by-environment",
        );
        std::env::set_var(
            "__NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_RELATIVE_OVERRIDDEN",
            "test-FAILED-value-set-by-environment",
        );

        std::env::set_var(
            "__NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_IN_EXTRA",
            "test-FAILED-value-set-by-environment",
        );
        std::env::set_var(
            "__NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_IN_MAIN",
            "test-PASSED-value-set-by-environment",
        );
        std::env::set_var(
            "__NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_IN_BOTH",
            "test-FAILED-value-set-by-environment",
        );
        std::env::set_var(
            "__NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_NONE",
            "test-PASSED-value-set-by-environment",
        );
        std::env::set_var(
            "__NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_FALSE",
            "test-PASSED-value-set-by-environment",
        );

        // Remove OUT_DIR from the environment, as it interferes with tests
        // (some of them expect that OUT_DIR isn't set.)
        std::env::remove_var("OUT_DIR");
    }
}

pub(crate) fn workspace_root() -> Utf8PathBuf {
    // one level up from the manifest dir -> into fixtures/nextest-tests
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fixtures/nextest-tests")
}
