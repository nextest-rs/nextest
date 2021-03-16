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
