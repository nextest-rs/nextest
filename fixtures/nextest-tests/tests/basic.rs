// Copyright (c) The nextest Contributors
use std::{env, io::Read, path::Path};

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
fn test_flaky_mod_4() {
    // Use this undocumented environment variable to figure out how many times this test has been
    // run so far.
    let nextest_attempt = nextest_attempt();
    if nextest_attempt % 4 != 0 {
        panic!("Failed because attempt {} % 4 != 0", nextest_attempt)
    }
}

#[test]
fn test_flaky_mod_6() {
    // Use this undocumented environment variable to figure out how many times this test has been
    // run so far.
    let nextest_attempt = nextest_attempt();
    if nextest_attempt % 6 != 0 {
        panic!("Failed because attempt {} % 6 != 0", nextest_attempt)
    }
}

#[test]
#[should_panic]
fn test_success_should_panic() {
    panic!("this is really a success")
}

#[test]
#[should_panic(expected = "this is a panic message")]
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

/// Test that a binary can be successfully executed.
#[test]
fn test_execute_bin() {
    nextest_tests::test_execute_bin_helper();
}

macro_rules! assert_env {
    ($name: expr) => {
        assert_env!($name, $name);
    };
    ($compile_time_name: expr, $runtime_name: expr) => {
        let compile_time_env = env!($compile_time_name);
        let runtime_env = std::env::var($runtime_name).expect(concat!(
            "env var ",
            $runtime_name,
            " missing at runtime"
        ));
        println!(
            concat!(
                "for env var ",
                $compile_time_name,
                " (runtime ",
                $runtime_name,
                "), compile time value: {}, runtime: {}",
            ),
            compile_time_env, runtime_env
        );
        assert_eq!(
            compile_time_env, runtime_env,
            concat!(
                "env var ",
                $compile_time_name,
                " (runtime ",
                $runtime_name,
                ") same between compile time and runtime",
            ),
        );
    };
}

/// Assert that test environment variables are correctly set.
#[test]
fn test_cargo_env_vars() {
    for (k, v) in std::env::vars() {
        println!("{} = {}", k, v);
    }
    assert_eq!(
        std::env::var("NEXTEST").as_deref(),
        Ok("1"),
        "NEXTEST environment variable set to 1"
    );
    assert_eq!(
        std::env::var("NEXTEST_EXECUTION_MODE").as_deref(),
        Ok("process-per-test"),
        "NEXTEST_EXECUTION_MODE set to process-per-test"
    );
    // https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
    assert_env!("CARGO");
    assert_env!(
        "CARGO_MANIFEST_DIR",
        "__NEXTEST_ORIGINAL_CARGO_MANIFEST_DIR"
    );
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

#[test]
#[ignore]
fn test_slow_timeout() {
    // The timeout for the with-termination profile is set to 2 seconds.
    std::thread::sleep(std::time::Duration::from_secs(4));
}

#[test]
#[ignore]
fn test_slow_timeout_2() {
    // There's a per-test override for the with-termination profile for this test: it is set to 1 second.
    std::thread::sleep(std::time::Duration::from_millis(1500));
}

#[test]
fn test_result_failure() -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "this is an error",
    ))
}

#[cfg(any(unix, windows))]
#[test]
fn test_subprocess_doesnt_exit() {
    // Note: setting a high value here can cause large delays with the GitHub Actions runner on
    // Windows.
    let mut cmd = sleep_cmd(5);
    cmd.spawn().unwrap();
}

#[cfg(windows)]
fn sleep_cmd(secs: usize) -> std::process::Command {
    // Apparently, this is the most reliable way to sleep for a bit on Windows.
    // * "timeout" doesn't work in a non-console context such as GitHub Actions runners.
    // * "waitfor" requires uniquely-named signals.
    // * "ping" just works.
    let mut cmd = std::process::Command::new("ping");
    // secs + 1 because the first attempt happens at the start.
    let secs_str = format!("{}", secs + 1);
    cmd.args(["/n", secs_str.as_str(), "127.0.0.1"]);
    cmd
}

#[cfg(unix)]
fn sleep_cmd(secs: usize) -> std::process::Command {
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg(&format!("{secs}"));
    cmd
}

#[test]
fn test_stdin_closed() {
    let mut buf = [0u8; 8];
    // This should succeed with 0 because it's attached to /dev/null or its Windows equivalent.
    assert_eq!(
        0,
        std::io::stdin()
            .read(&mut buf)
            .expect("reading from /dev/null succeeded")
    );
}
