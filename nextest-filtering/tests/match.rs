// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use guppy::{
    PackageId,
    graph::{PackageGraph, cargo::BuildPlatform},
};
use nextest_filtering::{
    BinaryQuery, CompiledExpr, EvalContext, Filterset, FiltersetKind, ParseContext, TestQuery,
    errors::{FiltersetParseErrors, ParseSingleError},
};
use nextest_metadata::{RustBinaryId, RustTestBinaryKind, TestCaseName};
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

fn parse(input: &str, graph: &PackageGraph) -> Filterset {
    let cx = ParseContext::new(graph);
    let expr = Filterset::parse(input.to_owned(), &cx, FiltersetKind::Test).unwrap();
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

impl BinaryQueryCreator<'_> {
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

#[inline]
fn test_something() -> TestCaseName {
    TestCaseName::new("test_something")
}

#[inline]
fn test_parse() -> TestCaseName {
    TestCaseName::new("test_parse")
}

#[inline]
fn test_run() -> TestCaseName {
    TestCaseName::new("test_run")
}

#[inline]
fn test_build() -> TestCaseName {
    TestCaseName::new("test_build")
}

#[inline]
fn test_parse_set() -> TestCaseName {
    TestCaseName::new("test_parse_set")
}

#[inline]
fn test_parse_expr() -> TestCaseName {
    TestCaseName::new("test_parse_expr")
}

#[inline]
fn test_expr() -> TestCaseName {
    TestCaseName::new("test_expr")
}

#[test]
fn test_expr_package_contains() {
    let graph = load_graph();
    let expr = parse("package(~_a)", &graph);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
}

#[test]
fn test_expr_package_equal() {
    let graph = load_graph();
    let expr = parse("package(=crate_a)", &graph);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
}

#[test]
fn test_expr_package_regex() {
    let graph = load_graph();
    let expr = parse("package(/crate_(a|b)/)", &graph);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
}

#[test]
fn test_expr_binary_id_glob() {
    let graph = load_graph();
    let expr = parse("binary_id(crate_[ab])", &graph);
    println!("{expr:?}");

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let pid_c = mk_pid('c');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
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
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    // a-d are deps of d
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_d, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));

    // e-g are not deps of d
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_e, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_f, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_g, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
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
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };
    // a-c are not rdeps of d
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_c, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));

    // d-g are rdeps of d
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_d, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_e, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_f, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_g, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
}

#[test]
fn test_expr_with_no_matching_packages() {
    #[track_caller]
    fn assert_error(errors: &FiltersetParseErrors) {
        assert_eq!(errors.errors.len(), 1);
        assert!(matches!(
            errors.errors[0],
            ParseSingleError::NoPackageMatch(_)
        ));
    }

    let graph = load_graph();
    let cx = ParseContext::new(&graph);

    let errors =
        Filterset::parse("deps(does-not-exist)".to_owned(), &cx, FiltersetKind::Test).unwrap_err();
    assert_error(&errors);

    let errors =
        Filterset::parse("deps(=does-not-exist)".to_owned(), &cx, FiltersetKind::Test).unwrap_err();
    assert_error(&errors);

    let errors =
        Filterset::parse("deps(~does-not-exist)".to_owned(), &cx, FiltersetKind::Test).unwrap_err();
    assert_error(&errors);

    let errors =
        Filterset::parse("deps(/does-not/)".to_owned(), &cx, FiltersetKind::Test).unwrap_err();
    assert_error(&errors);
}

#[test]
fn test_expr_kind() {
    let graph = load_graph();
    let expr = parse("kind(lib)", &graph);

    let pid_a = mk_pid('a');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "test", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib2", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
}

#[test]
fn test_expr_binary() {
    let graph = load_graph();
    let expr = parse("binary(crate_f)", &graph);

    let pid_a = mk_pid('a');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "crate_f", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(
        !expr.matches_test(
            &TestQuery {
                binary_query: binary_query(
                    &graph,
                    &pid_a,
                    "test",
                    "my-binary2",
                    BuildPlatform::Target
                )
                .to_query(),
                test_name: &test_parse()
            },
            &cx
        )
    );
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib2", "crate_f", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
}

#[test]
fn test_expr_platform() {
    let graph = load_graph();
    let expr = parse("platform(host)", &graph);

    let pid_a = mk_pid('a');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Host).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));

    let expr = parse("platform(target)", &graph);

    let pid_a = mk_pid('a');
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Host).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
}

#[test]
fn test_expr_kind_partial() {
    let graph = load_graph();
    let expr = parse("kind(~tes)", &graph);

    let pid_a = mk_pid('a');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "test", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_something()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse()
        },
        &cx
    ));
}

#[test]
fn test_expr_test() {
    let graph = load_graph();
    let expr = parse("test(parse)", &graph);

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_b, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_run()
        },
        &cx
    ));
}

#[test]
fn test_expr_test_not() {
    let graph = load_graph();
    let expr = parse("not test(parse)", &graph);

    let pid_a = mk_pid('a');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_run()
        },
        &cx
    ));
}

#[test_case("test(parse) + test(run)"; "with plus")]
#[test_case("test(parse) | test(run)"; "with pipe")]
#[test_case("test(parse) or test(run)"; "with or")]
fn test_expr_test_union(input: &str) {
    let graph = load_graph();
    let expr = parse(input, &graph);

    let pid_a = mk_pid('a');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_run()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_build()
        },
        &cx
    ));
}

#[test_case("test(parse) - test(expr)"; "with minus")]
#[test_case("test(parse) and not test(expr)"; "with and not")]
fn test_expr_test_difference(input: &str) {
    let graph = load_graph();
    let expr = parse(input, &graph);

    let pid_a = mk_pid('a');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse_set()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse_expr()
        },
        &cx
    ));
}

#[test_case("test(parse) & test(expr)"; "with ampersand")]
#[test_case("test(parse) and test(expr)"; "with and")]
fn test_expr_test_intersect(input: &str) {
    let graph = load_graph();
    let expr = parse(input, &graph);

    let pid_a = mk_pid('a');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse()
        },
        &cx
    ));
    assert!(!expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_expr()
        },
        &cx
    ));
    assert!(expr.matches_test(
        &TestQuery {
            binary_query:
                binary_query(&graph, &pid_a, "lib", "my-binary", BuildPlatform::Target).to_query(),
            test_name: &test_parse_expr()
        },
        &cx
    ));
}

#[test]
fn test_binary_query() {
    let graph = load_graph();
    let expr = parse(
        "binary(crate_a) + !platform(target) + kind(bench) + (package(~_a) & (!test(/foo/) | kind(bin)))",
        &graph,
    );

    let pid_a = mk_pid('a');
    let pid_b = mk_pid('b');
    let cx = EvalContext {
        default_filter: &CompiledExpr::ALL,
    };

    // binary = crate_a should match the first predicate (pid_a should not be relevant).
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_a, "lib", "crate_a", BuildPlatform::Target).to_query(),
            &cx,
        ),
        Some(true)
    );
    // platform = host should match the second predicate.
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_b, "lib", "bar", BuildPlatform::Host).to_query(),
            &cx,
        ),
        Some(true)
    );
    // kind = bench should match the third predicate.
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_b, "bench", "baz", BuildPlatform::Target).to_query(),
            &cx,
        ),
        Some(true)
    );
    // This should result in an unknown result since it involves a test predicate.
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_a, "lib", "baz", BuildPlatform::Target,).to_query(),
            &cx,
        ),
        None
    );
    // This should not result in an unknown result because no matter what the test predicate is,
    // kind(bin) resolves to true.
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_a, "bin", "baz", BuildPlatform::Target,).to_query(),
            &cx,
        ),
        Some(true)
    );
    // This should result in Some(false) since it doesn't match anything.
    assert_eq!(
        expr.matches_binary(
            &binary_query(&graph, &pid_b, "lib", "baz", BuildPlatform::Target,).to_query(),
            &cx,
        ),
        Some(false)
    );
}
