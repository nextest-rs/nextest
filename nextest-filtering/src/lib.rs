// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashSet;

use guppy::{
    graph::{DependsCache, PackageGraph, PackageMetadata},
    PackageId,
};

pub mod error;
mod parsing;

use parsing::{Expr, SetDef};

/// Matcher for name
///
/// Used both for package name and test name
#[derive(Debug, Clone)]
pub enum NameMatcher {
    /// Exact value
    Equal(String),
    /// Simple contains test
    Contains(String),
    /// Test against a regex
    Regex(regex::Regex),
}

impl PartialEq for NameMatcher {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Contains(s1), Self::Contains(s2)) => s1 == s2,
            (Self::Equal(s1), Self::Equal(s2)) => s1 == s2,
            (Self::Regex(r1), Self::Regex(r2)) => r1.as_str() == r2.as_str(),
            _ => false,
        }
    }
}

impl Eq for NameMatcher {}

/// Define a set of tests
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilteringSet {
    /// All tests in packages
    Packages(HashSet<PackageId>),
    /// All tests matching a name
    Test(NameMatcher),
    /// All tests
    All,
    /// No tests
    None,
    // Possible addition: Binary(NameMatcher)
}

/// Filtering expression
///
/// Used to filter tests to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilteringExpr {
    /// Accepts every test not in the given expression
    Not(Box<FilteringExpr>),
    /// Accepts every test in either given expression
    Union(Box<FilteringExpr>, Box<FilteringExpr>),
    /// Accepts every test in both given expressions
    Intersection(Box<FilteringExpr>, Box<FilteringExpr>),
    /// Accepts every test in a set
    Set(FilteringSet),
}

impl NameMatcher {
    pub fn is_match(&self, input: &str) -> bool {
        match self {
            Self::Equal(text) => text == input,
            Self::Contains(text) => input.contains(text),
            Self::Regex(reg) => reg.is_match(input),
        }
    }
}

impl FilteringSet {
    fn includes(&self, package_id: &PackageId, name: &str) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Test(matcher) => matcher.is_match(name),
            Self::Packages(packages) => packages.contains(package_id),
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

impl FilteringExpr {
    /// Parse a filtering expression
    pub fn parse(input: &str, graph: &PackageGraph) -> Result<FilteringExpr, error::Error> {
        let info = nom_tracable::TracableInfo::new();
        match parsing::parse_expression(parsing::Span::new_extra(input, info)) {
            Ok(expr) => {
                let in_workspace_packages: Vec<_> =
                    graph.packages().filter(|p| p.in_workspace()).collect();
                let mut cache = graph.new_depends_cache();
                Ok(compile_expr(&expr, &in_workspace_packages, &mut cache))
            }
            Err(_) => Err(error::Error::Failed(input.to_string())),
        }
    }

    /// Returns true if the given test is accepted by this filter
    pub fn includes(&self, package_id: &PackageId, name: &str) -> bool {
        match self {
            Self::Set(set) => set.includes(package_id, name),
            Self::Not(expr) => !expr.includes(package_id, name),
            Self::Union(expr_1, expr_2) => {
                expr_1.includes(package_id, name) || expr_2.includes(package_id, name)
            }
            Self::Intersection(expr_1, expr_2) => {
                expr_1.includes(package_id, name) && expr_2.includes(package_id, name)
            }
        }
    }
}
