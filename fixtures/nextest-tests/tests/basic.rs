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
