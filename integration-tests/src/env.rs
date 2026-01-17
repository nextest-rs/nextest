// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8PathBuf;

/// Environment info captured before sanitization.
#[derive(Debug)]
pub struct TestEnvInfo {
    /// Path to the cargo-nextest-dup binary.
    pub cargo_nextest_dup_bin: Utf8PathBuf,
    /// Path to the fake_interceptor binary.
    pub fake_interceptor_bin: Utf8PathBuf,
    /// Path to the rustc_shim binary.
    pub rustc_shim_bin: Utf8PathBuf,
    /// Path to the passthrough binary.
    pub passthrough_bin: Utf8PathBuf,
    /// Path to the grab_foreground binary (Unix only).
    #[cfg(unix)]
    pub grab_foreground_bin: Utf8PathBuf,
}

/// Sets up environment variables for a setup script.
///
/// Setup scripts don't have access to `NEXTEST_BIN_EXE_*` variables, so this
/// only performs sanitization without capturing binary paths.
pub fn set_env_vars_for_script() {
    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe {
        sanitize_env();
    }
}

/// Sets up environment variables for a test.
///
/// This captures binary paths from `NEXTEST_BIN_EXE_*` variables before
/// sanitizing the environment.
#[track_caller]
pub fn set_env_vars_for_test() -> TestEnvInfo {
    // Capture required binary paths before sanitization.
    let cargo_nextest_dup_bin: Utf8PathBuf = std::env::var("NEXTEST_BIN_EXE_cargo_nextest_dup")
        .expect("NEXTEST_BIN_EXE_cargo_nextest_dup should be set")
        .into();
    let fake_interceptor_bin: Utf8PathBuf = std::env::var("NEXTEST_BIN_EXE_fake_interceptor")
        .expect("NEXTEST_BIN_EXE_fake_interceptor should be set")
        .into();
    let rustc_shim_bin: Utf8PathBuf = std::env::var("NEXTEST_BIN_EXE_rustc_shim")
        .expect("NEXTEST_BIN_EXE_rustc_shim should be set")
        .into();
    let passthrough_bin: Utf8PathBuf = std::env::var("NEXTEST_BIN_EXE_passthrough")
        .expect("NEXTEST_BIN_EXE_passthrough should be set")
        .into();
    #[cfg(unix)]
    let grab_foreground_bin: Utf8PathBuf = std::env::var("NEXTEST_BIN_EXE_grab_foreground")
        .expect("NEXTEST_BIN_EXE_grab_foreground should be set")
        .into();

    // Ensure NEXTEST_PROFILE is set (we're running under nextest).
    std::env::var("NEXTEST_PROFILE").expect("NEXTEST_PROFILE should be set");

    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe {
        sanitize_env();
    }

    TestEnvInfo {
        cargo_nextest_dup_bin,
        fake_interceptor_bin,
        rustc_shim_bin,
        passthrough_bin,
        #[cfg(unix)]
        grab_foreground_bin,
    }
}

/// Sanitizes the environment by removing `NEXTEST_*` and `CARGO_*` variables
/// and setting up variables needed for integration tests.
///
/// # Safety
///
/// This function modifies the process environment, which is not thread-safe.
/// See <https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests>.
unsafe fn sanitize_env() {
    // Sanitize the environment by removing all NEXTEST_* and CARGO_* variables
    // from the parent environment. This ensures deterministic behavior regardless
    // of what the parent process has set. We'll then set specific variables below.

    // Collect keys first to avoid mutating while iterating.
    let keys_to_remove: Vec<_> = std::env::vars()
        .filter(|(key, _)| key.starts_with("NEXTEST_") || key.starts_with("CARGO_"))
        .map(|(key, _)| key)
        .collect();
    for key in keys_to_remove {
        std::env::remove_var(&key);
    }

    // The dynamic library tests require this flag.
    std::env::set_var("RUSTFLAGS", "-C prefer-dynamic");

    // Set CARGO_TERM_COLOR to never to ensure that ANSI color codes don't
    // interfere with the output.
    std::env::set_var("CARGO_TERM_COLOR", "never");

    // This environment variable is required to test the #[bench] fixture.
    // Note that THIS IS FOR TEST CODE ONLY. NEVER USE THIS IN PRODUCTION.
    std::env::set_var("RUSTC_BOOTSTRAP", "1");

    // Disable the tests which check for environment variables being set in
    // `config.toml`, as they won't be in the search path when running
    // integration tests.
    std::env::set_var("__NEXTEST_NO_CHECK_CARGO_ENV_VARS", "1");

    // Display empty STDOUT and STDERR lines in the output of failed tests.
    // This allows tests which make sure outputs are being displayed to
    // work.
    std::env::set_var("__NEXTEST_DISPLAY_EMPTY_OUTPUTS", "1");

    // Remove OUT_DIR from the environment, as it interferes with tests
    // (some of them expect that OUT_DIR isn't set.)
    std::env::remove_var("OUT_DIR");

    // Set NEXTEST_SHOW_PROGRESS to counter to ensure user config doesn't
    // affect test output.
    std::env::set_var("NEXTEST_SHOW_PROGRESS", "counter");

    // Skip user config loading entirely for test isolation. This prevents
    // the user's personal config from affecting test results. (Note that
    // some config tests pass in --user-config-file, which overrides this
    // environment variable.)
    std::env::set_var("NEXTEST_USER_CONFIG_FILE", "none");
}
