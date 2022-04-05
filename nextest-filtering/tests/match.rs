// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use guppy::PackageId;
use nextest_filtering::FilteringExpr;

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
    let expr = FilteringExpr::parse("package(_a)", &graph).unwrap();
    println!("{:?}", expr);

    assert!(expr.includes(&mk_pid('a'), "test_something"));
    assert!(!expr.includes(&mk_pid('b'), "test_something"));
    assert!(!expr.includes(&mk_pid('c'), "test_something"));
    assert!(!expr.includes(&mk_pid('d'), "test_something"));
    assert!(!expr.includes(&mk_pid('e'), "test_something"));
    assert!(!expr.includes(&mk_pid('f'), "test_something"));
    assert!(!expr.includes(&mk_pid('g'), "test_something"));
}

#[test]
fn test_expr_package_equal() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("package(=crate_a)", &graph).unwrap();
    println!("{:?}", expr);

    assert!(expr.includes(&mk_pid('a'), "test_something"));
    assert!(!expr.includes(&mk_pid('b'), "test_something"));
    assert!(!expr.includes(&mk_pid('c'), "test_something"));
    assert!(!expr.includes(&mk_pid('d'), "test_something"));
    assert!(!expr.includes(&mk_pid('e'), "test_something"));
    assert!(!expr.includes(&mk_pid('f'), "test_something"));
    assert!(!expr.includes(&mk_pid('g'), "test_something"));
}

#[test]
fn test_expr_package_regex() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("package(/crate_(a|b)/)", &graph).unwrap();
    println!("{:?}", expr);

    assert!(expr.includes(&mk_pid('a'), "test_something"));
    assert!(expr.includes(&mk_pid('b'), "test_something"));
    assert!(!expr.includes(&mk_pid('c'), "test_something"));
    assert!(!expr.includes(&mk_pid('d'), "test_something"));
    assert!(!expr.includes(&mk_pid('e'), "test_something"));
    assert!(!expr.includes(&mk_pid('f'), "test_something"));
    assert!(!expr.includes(&mk_pid('g'), "test_something"));
}

#[test]
fn test_expr_deps() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("deps(crate_d)", &graph).unwrap();
    println!("{:?}", expr);

    assert!(expr.includes(&mk_pid('a'), "test_something"));
    assert!(expr.includes(&mk_pid('b'), "test_something"));
    assert!(expr.includes(&mk_pid('c'), "test_something"));
    assert!(expr.includes(&mk_pid('d'), "test_something"));
    assert!(!expr.includes(&mk_pid('e'), "test_something"));
    assert!(!expr.includes(&mk_pid('f'), "test_something"));
    assert!(!expr.includes(&mk_pid('g'), "test_something"));
}

#[test]
fn test_expr_rdeps() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("rdeps(crate_d)", &graph).unwrap();
    println!("{:?}", expr);

    assert!(!expr.includes(&mk_pid('a'), "test_something"));
    assert!(!expr.includes(&mk_pid('b'), "test_something"));
    assert!(!expr.includes(&mk_pid('c'), "test_something"));
    assert!(expr.includes(&mk_pid('d'), "test_something"));
    assert!(expr.includes(&mk_pid('e'), "test_something"));
    assert!(expr.includes(&mk_pid('f'), "test_something"));
    assert!(expr.includes(&mk_pid('g'), "test_something"));
}

#[test]
fn test_expr_test() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("test(parse)", &graph).unwrap();
    println!("{:?}", expr);
    assert!(expr.includes(&mk_pid('a'), "test_parse"));
    assert!(expr.includes(&mk_pid('b'), "test_parse"));
    assert!(!expr.includes(&mk_pid('a'), "test_run"));
}

#[test]
fn test_expr_test_not() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("not test(parse)", &graph).unwrap();
    println!("{:?}", expr);
    assert!(!expr.includes(&mk_pid('a'), "test_parse"));
    assert!(expr.includes(&mk_pid('a'), "test_run"));
}

#[test]
fn test_expr_test_union() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("test(parse) + test(run)", &graph).unwrap();
    println!("{:?}", expr);
    assert!(expr.includes(&mk_pid('a'), "test_parse"));
    assert!(expr.includes(&mk_pid('a'), "test_run"));
    assert!(!expr.includes(&mk_pid('a'), "test_build"));

    let expr = FilteringExpr::parse("test(parse) | test(run)", &graph).unwrap();
    println!("{:?}", expr);
    assert!(expr.includes(&mk_pid('a'), "test_parse"));
    assert!(expr.includes(&mk_pid('a'), "test_run"));
    assert!(!expr.includes(&mk_pid('a'), "test_build"));

    let expr = FilteringExpr::parse("test(parse) or test(run)", &graph).unwrap();
    println!("{:?}", expr);
    assert!(expr.includes(&mk_pid('a'), "test_parse"));
    assert!(expr.includes(&mk_pid('a'), "test_run"));
    assert!(!expr.includes(&mk_pid('a'), "test_build"));
}

#[test]
fn test_expr_test_difference() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("test(parse) - test(expr)", &graph).unwrap();
    println!("{:?}", expr);
    assert!(expr.includes(&mk_pid('a'), "test_parse"));
    assert!(expr.includes(&mk_pid('a'), "test_parse_set"));
    assert!(!expr.includes(&mk_pid('a'), "test_parse_expr"));

    let expr = FilteringExpr::parse("test(parse) & not test(expr)", &graph).unwrap();
    println!("{:?}", expr);
    assert!(expr.includes(&mk_pid('a'), "test_parse"));
    assert!(expr.includes(&mk_pid('a'), "test_parse_set"));
    assert!(!expr.includes(&mk_pid('a'), "test_parse_expr"));
}

#[test]
fn test_expr_test_intersect() {
    let graph = load_graph();
    let expr = FilteringExpr::parse("test(parse) & test(expr)", &graph).unwrap();
    println!("{:?}", expr);
    assert!(!expr.includes(&mk_pid('a'), "test_parse"));
    assert!(!expr.includes(&mk_pid('a'), "test_expr"));
    assert!(expr.includes(&mk_pid('a'), "test_parse_expr"));

    let expr = FilteringExpr::parse("test(parse) and test(expr)", &graph).unwrap();
    println!("{:?}", expr);
    assert!(!expr.includes(&mk_pid('a'), "test_parse"));
    assert!(!expr.includes(&mk_pid('a'), "test_expr"));
    assert!(expr.includes(&mk_pid('a'), "test_parse_expr"));
}
