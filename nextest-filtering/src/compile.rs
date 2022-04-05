// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use guppy::{
    graph::{DependsCache, PackageGraph, PackageMetadata},
    PackageId,
};
use std::collections::HashSet;

use crate::{
    expression::*,
    parsing::{Expr, SetDef},
};

pub(crate) fn compile(expr: &Expr, graph: &PackageGraph) -> FilteringExpr {
    let in_workspace_packages: Vec<_> = graph
        .resolve_workspace()
        .packages(guppy::graph::DependencyDirection::Forward)
        .collect();
    let mut cache = graph.new_depends_cache();
    compile_expr(expr, &in_workspace_packages, &mut cache)
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
) -> FilteringSet {
    match set {
        SetDef::Package(matcher) => FilteringSet::Packages(matching_packages(matcher, packages)),
        SetDef::Deps(matcher) => {
            FilteringSet::Packages(dependencies_packages(matcher, packages, cache))
        }
        SetDef::Rdeps(matcher) => {
            FilteringSet::Packages(rdependencies_packages(matcher, packages, cache))
        }
        SetDef::Test(matcher) => FilteringSet::Test(matcher.clone()),
        SetDef::All => FilteringSet::All,
        SetDef::None => FilteringSet::None,
    }
}

fn compile_expr(
    expr: &Expr,
    packages: &[PackageMetadata],
    cache: &mut DependsCache,
) -> FilteringExpr {
    match expr {
        Expr::Set(set) => FilteringExpr::Set(compile_set_def(set, packages, cache)),
        Expr::Not(expr) => FilteringExpr::Not(Box::new(compile_expr(expr, packages, cache))),
        Expr::Union(expr_1, expr_2) => FilteringExpr::Union(
            Box::new(compile_expr(expr_1, packages, cache)),
            Box::new(compile_expr(expr_2, packages, cache)),
        ),
        Expr::Intersection(expr_1, expr_2) => FilteringExpr::Intersection(
            Box::new(compile_expr(expr_1, packages, cache)),
            Box::new(compile_expr(expr_2, packages, cache)),
        ),
    }
}
