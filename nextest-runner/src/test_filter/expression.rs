// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::ParseFilterExprError, list::RustTestArtifact,
    test_filter::expression_parsing::parse_expression,
};

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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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

impl NameMatcher {
    pub fn is_match(&self, input: &str) -> bool {
        match self {
            Self::Equal(text) => text == input,
            Self::Contains(text) => input.contains(text),
            Self::Regex(reg) => reg.is_match(input),
        }
    }
}

impl SetDef {
    fn includes(&self, artifact: &RustTestArtifact<'_>, name: &str) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Test(matcher) => matcher.is_match(name),
            Self::Package(matcher) => matcher.is_match(artifact.package.name()),
            Self::Deps(_matcher) => unimplemented!("deps not implemented"),
            Self::Rdeps(_matcher) => unimplemented!("rdeps not implemented"),
        }
    }
}

impl Expr {
    pub fn parse(input: &str) -> Result<Expr, ParseFilterExprError> {
        let info = nom_tracable::TracableInfo::new();
        match parse_expression(super::expression_parsing::Span::new_extra(input, info)) {
            Ok(expr) => Ok(expr),
            Err(_) => Err(ParseFilterExprError::Failed(input.to_string())),
        }
    }

    pub fn includes(&self, artifact: &RustTestArtifact<'_>, name: &str) -> bool {
        match self {
            Self::Set(set) => set.includes(artifact, name),
            Self::Not(expr) => !expr.includes(artifact, name),
            Self::Union(expr_1, expr_2) => {
                expr_1.includes(artifact, name) || expr_2.includes(artifact, name)
            }
            Self::Intersection(expr_1, expr_2) => {
                expr_1.includes(artifact, name) && expr_2.includes(artifact, name)
            }
        }
    }
}
