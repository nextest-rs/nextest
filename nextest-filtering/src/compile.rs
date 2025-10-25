// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::{BannedPredicateReason, ParseSingleError},
    expression::*,
    parsing::{ParsedExpr, ParsedLeaf},
};
use guppy::{
    PackageId,
    graph::{DependsCache, PackageMetadata},
};
use miette::SourceSpan;
use recursion::CollapsibleExt;
use smol_str::SmolStr;
use std::collections::HashSet;

pub(crate) fn compile(
    expr: &ParsedExpr,
    cx: &ParseContext<'_>,
    kind: FiltersetKind,
) -> Result<CompiledExpr, Vec<ParseSingleError>> {
    let mut errors = vec![];
    check_banned_predicates(expr, kind, &mut errors);

    let cx_cache = cx.make_cache();
    let mut cache = cx.graph().new_depends_cache();
    let expr = compile_expr(expr, cx_cache, &mut cache, &mut errors);

    if errors.is_empty() {
        Ok(expr)
    } else {
        Err(errors)
    }
}

fn check_banned_predicates(
    expr: &ParsedExpr,
    kind: FiltersetKind,
    errors: &mut Vec<ParseSingleError>,
) {
    match kind {
        FiltersetKind::Test => {}
        FiltersetKind::TestArchive => {
            // The `test` predicate is unsupported for a test archive since we need to
            // package the whole binary and it may be cross-compiled.
            Wrapped(expr).collapse_frames(|layer: ExprFrame<&ParsedLeaf, ()>| {
                if let ExprFrame::Set(ParsedLeaf::Test(_, span)) = layer {
                    errors.push(ParseSingleError::BannedPredicate {
                        kind,
                        span: *span,
                        reason: BannedPredicateReason::Unsupported,
                    });
                }
            })
        }
        FiltersetKind::DefaultFilter => {
            // The `default` predicate is banned.
            Wrapped(expr).collapse_frames(|layer: ExprFrame<&ParsedLeaf, ()>| {
                if let ExprFrame::Set(ParsedLeaf::Default(span)) = layer {
                    errors.push(ParseSingleError::BannedPredicate {
                        kind,
                        span: *span,
                        reason: BannedPredicateReason::InfiniteRecursion,
                    });
                }
            })
        }
    }
}

fn matching_packages(
    matcher: &NameMatcher,
    all_packages: &[PackageMetadata<'_>],
) -> HashSet<PackageId> {
    all_packages
        .iter()
        .filter(|p| matcher.is_match(p.name()))
        .map(|p| p.id().clone())
        .collect()
}

fn dependencies_packages(
    matcher: &NameMatcher,
    all_packages: &[PackageMetadata<'_>],
    cache: &mut DependsCache<'_>,
) -> HashSet<PackageId> {
    let packages = all_packages
        .iter()
        .filter(|p| matcher.is_match(p.name()))
        .map(|p| p.id());
    let mut set = HashSet::new();
    for id1 in packages {
        for p2 in all_packages {
            let id2 = p2.id();
            if cache.depends_on(id1, id2).unwrap_or(false) {
                set.insert(id2.clone());
            }
        }
    }
    set
}

fn rdependencies_packages(
    matcher: &NameMatcher,
    all_packages: &[PackageMetadata<'_>],
    cache: &mut DependsCache<'_>,
) -> HashSet<PackageId> {
    let packages = all_packages
        .iter()
        .filter(|p| matcher.is_match(p.name()))
        .map(|p| p.id());
    let mut set = HashSet::new();
    for id1 in packages {
        for p2 in all_packages {
            let id2 = p2.id();
            if cache.depends_on(id2, id1).unwrap_or(false) {
                set.insert(id2.clone());
            }
        }
    }
    set
}

fn compile_set_def(
    set: &ParsedLeaf,
    cx_cache: &ParseContextCache<'_>,
    cache: &mut DependsCache<'_>,
    errors: &mut Vec<ParseSingleError>,
) -> FiltersetLeaf {
    match set {
        ParsedLeaf::Package(matcher, span) => FiltersetLeaf::Packages(expect_non_empty_packages(
            matching_packages(matcher, &cx_cache.workspace_packages),
            *span,
            errors,
        )),
        ParsedLeaf::Deps(matcher, span) => FiltersetLeaf::Packages(expect_non_empty_packages(
            dependencies_packages(matcher, &cx_cache.workspace_packages, cache),
            *span,
            errors,
        )),
        ParsedLeaf::Rdeps(matcher, span) => FiltersetLeaf::Packages(expect_non_empty_packages(
            rdependencies_packages(matcher, &cx_cache.workspace_packages, cache),
            *span,
            errors,
        )),
        ParsedLeaf::Kind(matcher, span) => FiltersetLeaf::Kind(matcher.clone(), *span),
        ParsedLeaf::Binary(matcher, span) => FiltersetLeaf::Binary(
            expect_non_empty_binary_names(matcher, &cx_cache.binary_names, *span, errors),
            *span,
        ),
        ParsedLeaf::BinaryId(matcher, span) => FiltersetLeaf::BinaryId(
            expect_non_empty_binary_ids(matcher, &cx_cache.binary_ids, *span, errors),
            *span,
        ),
        ParsedLeaf::Platform(platform, span) => FiltersetLeaf::Platform(*platform, *span),
        ParsedLeaf::Test(matcher, span) => FiltersetLeaf::Test(matcher.clone(), *span),
        ParsedLeaf::Default(_) => FiltersetLeaf::Default,
        ParsedLeaf::All => FiltersetLeaf::All,
        ParsedLeaf::None => FiltersetLeaf::None,
    }
}

fn expect_non_empty_packages(
    packages: HashSet<PackageId>,
    span: SourceSpan,
    errors: &mut Vec<ParseSingleError>,
) -> HashSet<PackageId> {
    if packages.is_empty() {
        errors.push(ParseSingleError::NoPackageMatch(span));
    }
    packages
}

fn expect_non_empty_binary_names(
    matcher: &NameMatcher,
    all_binary_names: &HashSet<&str>,
    span: SourceSpan,
    errors: &mut Vec<ParseSingleError>,
) -> NameMatcher {
    let any_matches = match matcher {
        NameMatcher::Equal { value, .. } => all_binary_names.contains(value.as_str()),
        _ => {
            // For anything more complex than equals, iterate over all the binary names.
            all_binary_names
                .iter()
                .any(|binary_name| matcher.is_match(binary_name))
        }
    };

    if !any_matches {
        errors.push(ParseSingleError::NoBinaryNameMatch(span));
    }
    matcher.clone()
}

fn expect_non_empty_binary_ids(
    matcher: &NameMatcher,
    all_binary_ids: &HashSet<SmolStr>,
    span: SourceSpan,
    errors: &mut Vec<ParseSingleError>,
) -> NameMatcher {
    let any_matches = match matcher {
        NameMatcher::Equal { value, .. } => all_binary_ids.contains(value.as_str()),
        _ => {
            // For anything more complex than equals, iterate over all the binary IDs.
            all_binary_ids
                .iter()
                .any(|binary_id| matcher.is_match(binary_id))
        }
    };

    if !any_matches {
        errors.push(ParseSingleError::NoBinaryIdMatch(span));
    }
    matcher.clone()
}

fn compile_expr(
    expr: &ParsedExpr,
    cx_cache: &ParseContextCache<'_>,
    cache: &mut DependsCache<'_>,
    errors: &mut Vec<ParseSingleError>,
) -> CompiledExpr {
    use crate::expression::ExprFrame::*;

    Wrapped(expr).collapse_frames(|layer: ExprFrame<&ParsedLeaf, CompiledExpr>| match layer {
        Set(set) => CompiledExpr::Set(compile_set_def(set, cx_cache, cache, errors)),
        Not(expr) => CompiledExpr::Not(Box::new(expr)),
        Union(expr_1, expr_2) => CompiledExpr::Union(Box::new(expr_1), Box::new(expr_2)),
        Intersection(expr_1, expr_2) => {
            CompiledExpr::Intersection(Box::new(expr_1), Box::new(expr_2))
        }
        Difference(expr_1, expr_2) => CompiledExpr::Intersection(
            Box::new(expr_1),
            Box::new(CompiledExpr::Not(Box::new(expr_2))),
        ),
        Parens(expr_1) => expr_1,
    })
}
