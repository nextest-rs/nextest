// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use guppy::{
    graph::{DependsCache, PackageGraph, PackageMetadata},
    PackageId,
};
use std::collections::HashSet;

use crate::error::{Error, FilteringExprParsingError};
use crate::expression::*;
use crate::parsing::{Expr, RawNameMatcher, SetDef};

pub(crate) fn compile(
    input: &str,
    raw_expr: &Expr,
    graph: &PackageGraph,
) -> Result<FilteringExpr, FilteringExprParsingError> {
    let in_workspace_packages: Vec<_> = graph
        .resolve_workspace()
        .packages(guppy::graph::DependencyDirection::Forward)
        .collect();
    let mut cache = graph.new_depends_cache();
    match compile_expr(raw_expr, &in_workspace_packages, &mut cache) {
        Some(expr) => Ok(expr),
        None => {
            // should not happen
            // This would only happen if the parse expression contains an Error variant,
            // in which case an error should had been push to the errors vec and we should already
            // have bail.
            // IMPROVE this is an internal error => add log to suggest opening an bug ?

            let err = Error::Unknown;
            let report = miette::Report::new(err).with_source_code(input.to_string());
            eprintln!("{:?}", report);
            Err(FilteringExprParsingError(input.to_string()))
        }
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
            if id1 == id2 {
                continue;
            }
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
            if id1 == id2 {
                continue;
            }
            if cache.depends_on(id2, id1).unwrap_or(false) {
                set.insert(id2.clone());
            }
        }
    }
    set
}

fn to_name_matcher(matcher: &RawNameMatcher) -> Option<NameMatcher> {
    match matcher {
        RawNameMatcher::Contains(t) => Some(NameMatcher::Contains(t.clone())),
        RawNameMatcher::Equal(t) => Some(NameMatcher::Equal(t.clone())),
        RawNameMatcher::Regex(r) => Some(NameMatcher::Regex(r.clone())),
        RawNameMatcher::Error => None,
    }
}

fn compile_set_def(
    set: &SetDef,
    packages: &[PackageMetadata],
    cache: &mut DependsCache,
) -> Option<FilteringSet> {
    match set {
        SetDef::Package(matcher) => Some(FilteringSet::Packages(matching_packages(
            &to_name_matcher(matcher)?,
            packages,
        ))),
        SetDef::Deps(matcher) => Some(FilteringSet::Packages(dependencies_packages(
            &to_name_matcher(matcher)?,
            packages,
            cache,
        ))),
        SetDef::Rdeps(matcher) => Some(FilteringSet::Packages(rdependencies_packages(
            &to_name_matcher(matcher)?,
            packages,
            cache,
        ))),
        SetDef::Test(matcher) => Some(FilteringSet::Test(to_name_matcher(matcher)?)),
        SetDef::All => Some(FilteringSet::All),
        SetDef::None => Some(FilteringSet::None),
        SetDef::Error => None,
    }
}

fn compile_expr(
    expr: &Expr,
    packages: &[PackageMetadata],
    cache: &mut DependsCache,
) -> Option<FilteringExpr> {
    match expr {
        Expr::Set(set) => Some(FilteringExpr::Set(compile_set_def(set, packages, cache)?)),
        Expr::Not(expr) => Some(FilteringExpr::Not(Box::new(compile_expr(
            expr, packages, cache,
        )?))),
        Expr::Union(expr_1, expr_2) => Some(FilteringExpr::Union(
            Box::new(compile_expr(expr_1, packages, cache)?),
            Box::new(compile_expr(expr_2, packages, cache)?),
        )),
        Expr::Intersection(expr_1, expr_2) => Some(FilteringExpr::Intersection(
            Box::new(compile_expr(expr_1, packages, cache)?),
            Box::new(compile_expr(expr_2, packages, cache)?),
        )),
        Expr::Error => None,
    }
}
