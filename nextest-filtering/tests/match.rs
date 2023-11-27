// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// test_case causes clippy warnings with Rust 1.71.
#![allow(clippy::items_after_test_module)]

use guppy::{
    graph::{cargo::BuildPlatform, PackageGraph},
    PackageId,
};
use nextest_filtering::{
    errors::{FilterExpressionParseErrors, ParseSingleError},
    BinaryQuery, FilteringExpr, TestQuery,
};
use nextest_metadata::{RustBinaryId, RustTestBinaryKind};
use test_case::test_case;

#[track_caller]
fn load_graph() -> guppy::graph::PackageGraph {
    let json = std::fs::read_to_string("../fixtures/tests-workspace-metadata.json").unwrap();
    guppy::CargoMetadata::parse_json(json)
        .unwrap()
        .build_graph()
        .unwrap()
}

fn mk_pid(c: char) -> PackageId {
    PackageId::new(format!(
        "crate_{c} 0.1.0 (path+file:///home/fakeuser/tests-workspace/crate-{c})"
    ))
}

fn parse(input: &str, graph: &PackageGraph) -> FilteringExpr {
    let expr = FilteringExpr::parse(input.to_owned(), graph).unwrap();
    eprintln!("expression: {expr:?}");
    expr
}

struct BinaryQueryCreator<'a> {
    package_id: &'a PackageId,
    binary_id: RustBinaryId,
    kind: RustTestBinaryKind,
    binary_name: &'a str,
    platform: BuildPlatform,
}

impl<'a> BinaryQueryCreator<'a> {
    fn to_query(&self) -> BinaryQuery<'_> {
        BinaryQuery {
            package_id: self.package_id,
            binary_id: &self.binary_id,
            kind: &self.kind,
            binary_name: self.binary_name,
            platform: self.platform,
        }
    }
}

fn binary_query<'a>(
    graph: &'a PackageGraph,
    package_id: &'a PackageId,
    kind: &str,
    binary_name: &'a str,
    platform: BuildPlatform,
) -> BinaryQueryCreator<'a> {
    let package_name = graph.metadata(package_id).unwrap().name();
    let kind = RustTestBinaryKind::new(kind.to_owned());
    let binary_id = RustBinaryId::from_parts(package_name, &kind, binary_name);
    BinaryQueryCreator {
        package_id,
        binary_id,
        kind,
        binary_name,
        platform,
    }
}

#[test]
fn test_expr_package_contains() {
    let graph = load_graph();
    let expr = parse("package(~_a)", &graph);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_package_equal() {
    let graph = load_graph();
    let expr = parse("package(=crate_a)", &graph);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_package_regex() {
    let graph = load_graph();
    let expr = parse("package(/crate_(a|b)/)", &graph);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_binary_id_glob() {
    let graph = load_graph();
    let expr = parse("binary_id(crate_[ab])", &graph);
    println!("{:?}", expr);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_deps() {
    let graph = load_graph();
    let expr = parse("deps(crate_d)", &graph);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    let pid_d = mk_pid('d');
    let pid_e = mk_pid('e');
    let pid_f = mk_pid('f');
    let pid_g = mk_pid('g');
    // a-d are deps of d
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_d, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));

    // e-g are not deps of d
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_e, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_f, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_g, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_rdeps() {
    let graph = load_graph();
    let expr = parse("rdeps(crate_d)", &graph);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    let pid_d = mk_pid('d');
    let pid_e = mk_pid('e');
    let pid_f = mk_pid('f');
    let pid_g = mk_pid('g');
    // a-c are not rdeps of d
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));

    // d-g are rdeps of d
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_d, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_e, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_f, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_g, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
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
    let errors = FilteringExpr::parse("deps(does-not-exist)".to_owned(), &graph).unwrap_err();
    assert_error(&errors);

    let errors = FilteringExpr::parse("deps(=does-not-exist)".to_owned(), &graph).unwrap_err();
    assert_error(&errors);

    let errors = FilteringExpr::parse("deps(~does-not-exist)".to_owned(), &graph).unwrap_err();
    assert_error(&errors);

    let errors = FilteringExpr::parse("deps(/does-not/)".to_owned(), &graph).unwrap_err();
    assert_error(&errors);
}

#[test]
fn test_expr_kind() {
    let graph = load_graph();
    let expr = parse("kind(lib)", &graph);

    let pid_a = mk_pid('a');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "test", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib2", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_binary() {
    let graph = load_graph();
    let expr = parse("binary(my-binary)", &graph);

    let pid_a = mk_pid('a');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "test", "my-binary2", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib2", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_platform() {
    let graph = load_graph();
    let expr = parse("platform(host)", &graph);

    let pid_a = mk_pid('a');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Host)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));

    let expr = parse("platform(target)", &graph);

    let pid_a = mk_pid('a');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Host)
            .to_query(),
        test_name: "test_something"
    }));
}

#[test]
fn test_expr_kind_partial() {
    let graph = load_graph();
    let expr = parse("kind(~tes)", &graph);

    let pid_a = mk_pid('a');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "test", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_something"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse"
    }));
}

#[test]
fn test_expr_test() {
    let graph = load_graph();
    let expr = parse("test(parse)", &graph);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');

    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_run"
    }));
}

#[test]
fn test_expr_test_not() {
    let graph = load_graph();
    let expr = parse("not test(parse)", &graph);

    let pid_a = mk_pid('a');
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_run"
    }));
}

#[test_case("test(parse) + test(run)"; "with plus")]
#[test_case("test(parse) | test(run)"; "with pipe")]
#[test_case("test(parse) or test(run)"; "with or")]
fn test_expr_test_union(input: &str) {
    let graph = load_graph();
    let expr = parse(input, &graph);

    let pid_a = mk_pid('a');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_run"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_build"
    }));
}

#[test_case("test(parse) - test(expr)"; "with minus")]
#[test_case("test(parse) and not test(expr)"; "with and not")]
fn test_expr_test_difference(input: &str) {
    let graph = load_graph();
    let expr = parse(input, &graph);

    let pid_a = mk_pid('a');
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse_set"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse_expr"
    }));
}

#[test_case("test(parse) & test(expr)"; "with ampersand")]
#[test_case("test(parse) and test(expr)"; "with and")]
fn test_expr_test_intersect(input: &str) {
    let graph = load_graph();
    let expr = parse(input, &graph);

    let pid_a = mk_pid('a');
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse"
    }));
    assert!(!expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_expr"
    }));
    assert!(expr.matches_test(&TestQuery {
        binary_query: binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target)
            .to_query(),
        test_name: "test_parse_expr"
    }));
}

#[test]
fn test_binary_query() {
    let graph = load_graph();
    let expr = parse(
        "binary(foo) + !platform(target) + kind(bench) + (package(~_a) & (!test(/foo/) | kind(bin)))",
        &graph,
    );

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    // binary = foo should match the first predicate (pid_a should not be relevant).
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_a, "lib", "foo", BuildPlatform::Target).to_query()
        ),
        Some(true)
    );
    // platform = host should match the second predicate.
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_b, "lib", "bar", BuildPlatform::Host).to_query()
        ),
        Some(true)
    );
    // kind = bench should match the third predicate.
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_b, "bench", "baz", BuildPlatform::Target).to_query()
        ),
        Some(true)
    );
    // This should result in an unknown result since it involves a test predicate.
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_a, "lib", "baz", BuildPlatform::Target,).to_query()
        ),
        None
    );
    // This should not result in an unknown result because no matter what the test predicate is,
    // kind(bin) resolves to true.
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_a, "bin", "baz", BuildPlatform::Target,).to_query()
        ),
        Some(true)
    );
    // This should result in Some(false) since it doesn't match anything.
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_b, "lib", "baz", BuildPlatform::Target,).to_query()
        ),
        Some(false)
    );
}
