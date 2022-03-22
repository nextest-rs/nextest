// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::ParseFilterExprError, list::RustTestArtifact,
    test_filter::expression_parsing::parse_expression,
};

/// Matcher for name
///
/// Used both for package name and test name
#[derive(Debug)]
pub enum NameMatcher {
    /// Exact value
    Equal(String),
    /// Simple contains test
    Contains(String),
    /// Test against a regex
    Regex(regex::Regex),
}

#[cfg(test)]
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

#[cfg(test)]
impl Eq for NameMatcher {}

/// Define a set of tests
#[cfg_attr(test, derive(PartialEq, Eq))]
#[derive(Debug)]
pub enum SetDef {
    /// All tests in a package
    Package(NameMatcher),
    /// All tests in a package dependencies
    Deps(NameMatcher),
    /// All tests in a package reverse dependencies
    Rdeps(NameMatcher),
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
#[cfg_attr(test, derive(PartialEq, Eq))]
#[derive(Debug)]
pub enum Expr {
    /// Accepts every tests not in the given expression
    Not(Box<Expr>),
    /// Accepts every tests in either given expression
    Union(Box<Expr>, Box<Expr>),
    /// Accepts every tests in both given expression
    Intersection(Box<Expr>, Box<Expr>),
    /// Accepts every tests in a set
    Set(SetDef),
}
