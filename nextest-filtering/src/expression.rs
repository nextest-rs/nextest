// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use guppy::{graph::PackageGraph, PackageId};
use std::{cell::RefCell, collections::HashSet};

use crate::error::{Error, FilteringExprParsingError, State};
use crate::parsing::{parse, Span};

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

impl FilteringExpr {
    /// Parse a filtering expression
    pub fn parse(
        input: &str,
        graph: &PackageGraph,
    ) -> Result<FilteringExpr, FilteringExprParsingError> {
        let errors = RefCell::new(Vec::new());
        match parse(Span::new_extra(input, State::new(&errors))) {
            Ok(parsed_expr) => {
                let errors = errors.into_inner();

                if !errors.is_empty() {
                    for err in errors {
                        let report = miette::Report::new(err).with_source_code(input.to_string());
                        eprintln!("{:?}", report);
                    }
                    return Err(FilteringExprParsingError(input.to_string()));
                }

                crate::compile::compile(input, &parsed_expr, graph)
            }
            Err(_) => {
                // should not happen
                // According to our parsing strategy we should never produce an Err(_)
                // IMPROVE this is an internal error => add log to suggest opening an bug ?

                let err = Error::Unknown;
                let report = miette::Report::new(err).with_source_code(input.to_string());
                eprintln!("{:?}", report);
                Err(FilteringExprParsingError(input.to_string()))
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
