// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::{cell::RefCell, collections::HashSet};

use guppy::{
    graph::{DependsCache, PackageGraph, PackageMetadata},
    PackageId,
};

pub mod error;
mod parsing;

use parsing::{Expr, RawNameMatcher, SetDef};

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

impl FilteringExpr {
    /// Parse a filtering expression
    pub fn parse(
        input: &str,
        graph: &PackageGraph,
    ) -> Result<FilteringExpr, error::FilteringExprParsingError> {
        let errors = RefCell::new(Vec::new());
        match parsing::parse(parsing::Span::new_extra(input, error::State::new(&errors))) {
            Ok(expr) => {
                let errors = errors.into_inner();

                if !errors.is_empty() {
                    for err in errors {
                        let report = miette::Report::new(err).with_source_code(input.to_string());
                        eprintln!("{:?}", report);
                    }
                    return Err(error::FilteringExprParsingError(input.to_string()));
                }

                let in_workspace_packages: Vec<_> =
                    graph.packages().filter(|p| p.in_workspace()).collect();
                let mut cache = graph.new_depends_cache();
                match compile_expr(&expr, &in_workspace_packages, &mut cache) {
                    Some(expr) => Ok(expr),
                    None => {
                        // should not happen
                        // This would only happen if the parse expression contains an Error variant,
                        // in which case an error should had been push to the errors vec and we should already
                        // have bail.
                        // IMPROVE this is an internal error => add log to suggest opening an bug ?

                        let err = error::Error::Unknown;
                        let report = miette::Report::new(err).with_source_code(input.to_string());
                        eprintln!("{:?}", report);
                        Err(error::FilteringExprParsingError(input.to_string()))
                    }
                }
            }
            Err(_) => {
                // should not happen
                // According to our parsing strategy we should never produce an Err(_)
                // IMPROVE this is an internal error => add log to suggest opening an bug ?

                let err = error::Error::Unknown;
                let report = miette::Report::new(err).with_source_code(input.to_string());
                eprintln!("{:?}", report);
                Err(error::FilteringExprParsingError(input.to_string()))
            }
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

    /// Returns true if the given expression needs dependencies information to work
    pub fn needs_deps(raw_expr: &str) -> bool {
        // the expression needs dependencies expression if it uses deps(..) or rdeps(..)
        raw_expr.contains("deps")
    }
}
