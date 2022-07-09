// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use guppy::PackageId;
use nextest_filtering::{
    errors::{FilterExpressionParseErrors, ParseSingleError},
    FilteringExpr, FilteringExprQuery,
};
use test_case::test_case;

#[track_caller]
fn load_graph() -> guppy::graph::PackageGraph {
    let json = std::fs::read_to_string("../fixtures/tests-workspace-metadata.json").unwrap();
    guppy::CargoMetadata::parse_json(&json)
        .unwrap()
        .build_graph()
        .unwrap()
}

fn mk_pid(c: char) -> PackageId {
    PackageId::new(format!(
        "crate_{} 0.1.0 (path+file:///home/fakeuser/tests-workspace/crate-{})",
        c, c
    ))
}

#[test]
fn test_expr_package_contains() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("package(~_a)", &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_b,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_c,
        kind: "lib",
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_package_equal() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("package(=crate_a)", &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_b,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_c,
        kind: "lib",
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_package_regex() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("package(/crate_(a|b)/)", &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_b,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_c,
        kind: "lib",
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_deps() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("deps(crate_d)", &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    let pid_d = mk_pid('d');
    let pid_e = mk_pid('e');
    let pid_f = mk_pid('f');
    let pid_g = mk_pid('g');
    // a-d are deps of d
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_b,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_c,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_d,
        kind: "lib",
        test_name: "test_something"
    }));

    // e-g are not deps of d
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_e,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_f,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_g,
        kind: "lib",
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_rdeps() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("rdeps(crate_d)", &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    let pid_d = mk_pid('d');
    let pid_e = mk_pid('e');
    let pid_f = mk_pid('f');
    let pid_g = mk_pid('g');
    // a-c are not rdeps of d
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_b,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_c,
        kind: "lib",
        test_name: "test_something"
    }));

    // d-g are rdeps of d
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_d,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_e,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_f,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_g,
        kind: "lib",
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_with_no_matching_packages() {
    #[track_caller]
    fn assert_error(errors: &FilterExpressionParseErrors) {
        assert_eq!(errors.errors.len(), 1);
        assert!(matches!(
            errors.errors[0],
            ParseSingleError::NoPackageMatch(_)
        ));
    }

    let graph = load_graph();
    let errors = FilteringExpr::parse("deps(does-not-exist)", &graph).unwrap_err();
    assert_error(&errors);

    let errors = FilteringExpr::parse("deps(=does-not-exist)", &graph).unwrap_err();
    assert_error(&errors);

    let errors = FilteringExpr::parse("deps(~does-not-exist)", &graph).unwrap_err();
    assert_error(&errors);

    let errors = FilteringExpr::parse("deps(/does-not/)", &graph).unwrap_err();
    assert_error(&errors);
}

#[test]
fn test_expr_kind() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("kind(lib)", &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "test",
        test_name: "test_parse"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib2",
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_kind_partial() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("kind(~tes)", &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "test",
        test_name: "test_something"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_parse"
    }));
}

#[test]
fn test_expr_test() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("test(parse)", &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');

    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_parse"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_b,
        kind: "lib",
        test_name: "test_parse"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_run"
    }));
}

#[test]
fn test_expr_test_not() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("not test(parse)", &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_parse"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_run"
    }));
}

#[test]
fn test_expr_binary() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("not test(parse)", &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    assert!(!expr.matches(&FilteringSetQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_parse"
    }));
    assert!(expr.matches(&FilteringSetQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_run"
    }));
}

#[test_case("test(parse) + test(run)"; "with plus")]
#[test_case("test(parse) | test(run)"; "with pipe")]
#[test_case("test(parse) or test(run)"; "with or")]
fn test_expr_test_union(input: &str) {
    let graph = load_graph();
    let expr = FilteringExpr::parse(input, &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_parse"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_run"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_build"
    }));
}

#[test_case("test(parse) - test(expr)"; "with minus")]
#[test_case("test(parse) and not test(expr)"; "with and not")]
fn test_expr_test_difference(input: &str) {
    let graph = load_graph();
    let expr = FilteringExpr::parse(input, &graph).unwrap();
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_parse"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_parse_set"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_parse_expr"
    }));
}

#[test_case("test(parse) & test(expr)"; "with ampersand")]
#[test_case("test(parse) and test(expr)"; "with and")]
fn test_expr_test_intersect(input: &str) {
    let graph = load_graph();
    let expr = FilteringExpr::parse(input, &graph).unwrap();
    println!("{:?}", expr);
    let pid_a = mk_pid('a');
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_parse"
    }));
    assert!(!expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_expr"
    }));
    assert!(expr.matches(&FilteringExprQuery {
        package_id: &pid_a,
        kind: "lib",
        test_name: "test_parse_expr"
    }));
}
