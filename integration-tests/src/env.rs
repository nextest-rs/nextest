// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[track_caller]
pub fn set_env_vars() {
    // The dynamic library tests require this flag.
    std::env::set_var("RUSTFLAGS", "-C prefer-dynamic");
    // Set CARGO_TERM_COLOR to never to ensure that ANSI color codes don't interfere with the
    // output.
    // TODO: remove this once programmatic run statuses are supported.
    std::env::set_var("CARGO_TERM_COLOR", "never");
    // This environment variable is required to test the #[bench] fixture. Note that THIS IS FOR
    // TEST CODE ONLY. NEVER USE THIS IN PRODUCTION.
    std::env::set_var("RUSTC_BOOTSTRAP", "1");

    // Disable the tests which check for environment variables being set in `config.toml`, as they
    // won't be in the search path when running integration tests.
    std::env::set_var("__NEXTEST_NO_CHECK_CARGO_ENV_VARS", "1");

    // Display empty STDOUT and STDERR lines in the output of failed tests. This
    // allows tests which make sure outputs are being displayed to work.
    std::env::set_var("__NEXTEST_DISPLAY_EMPTY_OUTPUTS", "1");

    // Remove OUT_DIR from the environment, as it interferes with tests (some of them expect that
    // OUT_DIR isn't set.)
    std::env::remove_var("OUT_DIR");
}
