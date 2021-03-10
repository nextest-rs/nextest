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
