// Copyright (c) The nextest Contributors
use std::{env, path::Path};

#[test]
fn test_success() {}

#[test]
fn test_failure_assert() {
    assert_eq!(2 + 2, 5, "this is an assertion")
}

#[test]
fn test_failure_error() -> Result<(), String> {
    return Err("this is an error".into());
}

fn nextest_attempt() -> usize {
    static NEXTEST_ATTEMPT_ENV: &str = "__NEXTEST_ATTEMPT";
    match env::var(NEXTEST_ATTEMPT_ENV) {
        Ok(var) => var
            .parse()
            .expect("__NEXTEST_ATTEMPT should be a positive integer"),
        Err(_) => 1,
    }
}

#[test]
fn test_flaky_mod_2() {
    // Use this undocumented environment variable to figure out how many times this test has been
    // run so far.
    let nextest_attempt = nextest_attempt();
    if nextest_attempt % 2 != 0 {
        panic!("Failed because attempt {} % 2 != 0", nextest_attempt)
    }
}

#[test]
fn test_flaky_mod_3() {
    // Use this undocumented environment variable to figure out how many times this test has been
    // run so far.
    let nextest_attempt = nextest_attempt();
    if nextest_attempt % 3 != 0 {
        panic!("Failed because attempt {} % 3 != 0", nextest_attempt)
    }
}

#[test]
#[should_panic]
fn test_success_should_panic() {
    panic!("this is really a success")
}

#[test]
#[should_panic]
fn test_failure_should_panic() {}

#[test]
fn test_cwd() {
    // Ensure that the cwd is correct.
    let runtime_cwd = env::current_dir().expect("should be able to read current dir");
    let compile_time_cwd = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(runtime_cwd, compile_time_cwd, "current dir matches");
}

#[test]
#[ignore]
fn test_ignored() {}

#[test]
#[ignore]
fn test_ignored_fail() {
    panic!("ignored test that fails");
}

macro_rules! assert_env {
    ($name: expr) => {
        let compile_time_env = env!($name);
        let runtime_env =
            std::env::var($name).expect(concat!("env var ", $name, " missing at runtime"));
        println!(
            concat!(
                "for env var ",
                $name,
                ", compile time value: {}, runtime: {}"
            ),
            compile_time_env, runtime_env
        );
        assert_eq!(
            compile_time_env, runtime_env,
            concat!("env var ", $name, " same between compile time and runtime"),
        );
    };
}

/// Assert that test environment variables are correctly set.
#[test]
fn test_cargo_env_vars() {
    assert_eq!(
        std::env::var("NEXTEST").as_deref(),
        Ok("1"),
        "NEXTEST environment variable set to 1"
    );
    // https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
    assert_env!("CARGO");
    assert_env!("CARGO_MANIFEST_DIR");
    assert_env!("CARGO_PKG_VERSION");
    assert_env!("CARGO_PKG_VERSION_MAJOR");
    assert_env!("CARGO_PKG_VERSION_MINOR");
    assert_env!("CARGO_PKG_VERSION_PATCH");
    assert_env!("CARGO_PKG_VERSION_PRE");
    assert_env!("CARGO_PKG_AUTHORS");
    assert_env!("CARGO_PKG_NAME");
    assert_env!("CARGO_PKG_DESCRIPTION");
    assert_env!("CARGO_PKG_HOMEPAGE");
    assert_env!("CARGO_PKG_REPOSITORY");
    assert_env!("CARGO_PKG_LICENSE");
    assert_env!("CARGO_PKG_LICENSE_FILE");
    // CARGO_CRATE_NAME is missing at runtime
    // CARGO_BIN_EXE is missing at runtime
    // CARGO_PRIMARY_PACKAGE is missing at runtime
    // CARGO_TARGET_TMPDIR is missing at runtime
    // TODO: dynamic library paths?
}
