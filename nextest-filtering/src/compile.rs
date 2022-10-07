// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::ParseSingleError,
    expression::*,
    parsing::{Expr, SetDef},
};
use guppy::{
    graph::{DependsCache, PackageGraph, PackageMetadata},
    PackageId,
};
use miette::SourceSpan;
use recursion::expand_and_collapse;
use std::collections::HashSet;

pub(crate) fn compile(
    expr: &Expr,
    graph: &PackageGraph,
) -> Result<BoxedFilteringExpr, Vec<ParseSingleError>> {
    let in_workspace_packages: Vec<_> = graph
        .resolve_workspace()
        .packages(guppy::graph::DependencyDirection::Forward)
        .collect();
    let mut cache = graph.new_depends_cache();
    let mut errors = vec![];
    let expr = compile_expr(expr, &in_workspace_packages, &mut cache, &mut errors);

    if errors.is_empty() {
        Ok(expr)
    } else {
        Err(errors)
    }
}

fn matching_packages(
    matcher: &NameMatcher,
    all_packages: &[PackageMetadata],
) -> HashSet<PackageId> {
    all_packages
        .iter()
        .filter(|p| matcher.is_match(p.name()))
        .map(|p| p.id().clone())
        .collect()
}

fn dependencies_packages(
    matcher: &NameMatcher,
    all_packages: &[PackageMetadata],
    cache: &mut DependsCache,
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
    all_packages: &[PackageMetadata],
    cache: &mut DependsCache,
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
    set: &SetDef,
    packages: &[PackageMetadata],
    cache: &mut DependsCache,
    errors: &mut Vec<ParseSingleError>,
) -> FilteringSet {
    match set {
        SetDef::Package(matcher, span) => FilteringSet::Packages(expect_non_empty(
            matching_packages(matcher, packages),
            *span,
            errors,
        )),
        SetDef::Deps(matcher, span) => FilteringSet::Packages(expect_non_empty(
            dependencies_packages(matcher, packages, cache),
            *span,
            errors,
        )),
        SetDef::Rdeps(matcher, span) => FilteringSet::Packages(expect_non_empty(
            rdependencies_packages(matcher, packages, cache),
            *span,
            errors,
        )),
        SetDef::Kind(matcher, span) => FilteringSet::Kind(matcher.clone(), *span),
        SetDef::Binary(matcher, span) => FilteringSet::Binary(matcher.clone(), *span),
        SetDef::Platform(platform, span) => FilteringSet::Platform(*platform, *span),
        SetDef::Test(matcher, span) => FilteringSet::Test(matcher.clone(), *span),
        SetDef::All => FilteringSet::All,
        SetDef::None => FilteringSet::None,
    }
}

fn expect_non_empty(
    packages: HashSet<PackageId>,
    span: SourceSpan,
    errors: &mut Vec<ParseSingleError>,
) -> HashSet<PackageId> {
    if packages.is_empty() {
        errors.push(ParseSingleError::NoPackageMatch(span));
    }
    packages
}

fn compile_expr(
    expr: &Expr,
    packages: &[PackageMetadata],
    cache: &mut DependsCache,
    errors: &mut Vec<ParseSingleError>,
) -> BoxedFilteringExpr {
    expand_and_collapse(expr, Expr::as_filtering_expr_layer, |layer| {
        BoxedFilteringExpr::new(layer.map_set(|set| compile_set_def(set, packages, cache, errors)))
    })
}
