// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::{FilterExpressionParseErrors, ParseSingleError, State},
    parsing::{parse, Expr, ParsedExpr, SetDef, Span},
};
use guppy::{
    graph::{cargo::BuildPlatform, PackageGraph},
    PackageId,
};
use miette::SourceSpan;
use recursion::{
    map_layer::{MapLayer, Project},
    Collapse,
};
use std::{cell::RefCell, collections::HashSet};

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
    /// All tests present in this kind of binary.
    Kind(NameMatcher, SourceSpan),
    /// The platform a test is built for.
    Platform(BuildPlatform, SourceSpan),
    /// All binaries matching a name
    Binary(NameMatcher, SourceSpan),
    /// All tests matching a name
    Test(NameMatcher, SourceSpan),
    /// All tests
    All,
    /// No tests
    None,
}

/// A query for a binary, passed into [`FilteringExpr::matches_binary`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct BinaryQuery<'a> {
    /// The package ID.
    pub package_id: &'a PackageId,

    /// The name of the binary.
    pub binary_name: &'a str,

    /// The kind of binary this test is (lib, test etc).
    pub kind: &'a str,

    /// The platform this test is built for.
    pub platform: BuildPlatform,
}

/// A query for a specific test, passed into [`FilteringExpr::matches_test`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TestQuery<'a> {
    /// The binary query.
    pub binary_query: BinaryQuery<'a>,

    /// The name of the test.
    pub test_name: &'a str,
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
    pub(crate) fn is_match(&self, input: &str) -> bool {
        match self {
            Self::Equal(text) => text == input,
            Self::Contains(text) => input.contains(text),
            Self::Regex(reg) => reg.is_match(input),
        }
    }
}

impl FilteringSet {
    fn matches_test(&self, query: &TestQuery<'_>) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Test(matcher, _) => matcher.is_match(query.test_name),
            Self::Binary(matcher, _) => matcher.is_match(query.binary_query.binary_name),
            Self::Platform(platform, _) => query.binary_query.platform == *platform,
            Self::Kind(matcher, _) => matcher.is_match(query.binary_query.kind),
            Self::Packages(packages) => packages.contains(query.binary_query.package_id),
        }
    }

    fn matches_binary(&self, query: &BinaryQuery<'_>) -> Option<bool> {
        match self {
            Self::All => Logic::top(),
            Self::None => Logic::bottom(),
            Self::Test(_, _) => None,
            Self::Binary(matcher, _) => Some(matcher.is_match(query.binary_name)),
            Self::Platform(platform, _) => Some(query.platform == *platform),
            Self::Kind(matcher, _) => Some(matcher.is_match(query.kind)),
            Self::Packages(packages) => Some(packages.contains(query.package_id)),
        }
    }
}

impl FilteringExpr {
    /// Parse a filtering expression
    pub fn parse(
        input: &str,
        graph: &PackageGraph,
    ) -> Result<FilteringExpr, FilterExpressionParseErrors> {
        let errors = RefCell::new(Vec::new());
        match parse(Span::new_extra(input, State::new(&errors))) {
            Ok(parsed_expr) => {
                let errors = errors.into_inner();

                if !errors.is_empty() {
                    return Err(FilterExpressionParseErrors::new(input, errors));
                }

                match parsed_expr {
                    ParsedExpr::Valid(expr) => crate::compile::compile(&expr, graph)
                        .map_err(|errors| FilterExpressionParseErrors::new(input, errors)),
                    _ => {
                        // should not happen
                        // If an ParsedExpr::Error is produced, we should also have an error inside
                        // errors and we should already have returned
                        // IMPROVE this is an internal error => add log to suggest opening an bug ?
                        Err(FilterExpressionParseErrors::new(
                            input,
                            vec![ParseSingleError::Unknown],
                        ))
                    }
                }
            }
            Err(_) => {
                // should not happen
                // According to our parsing strategy we should never produce an Err(_)
                // IMPROVE this is an internal error => add log to suggest opening an bug ?
                Err(FilterExpressionParseErrors::new(
                    input,
                    vec![ParseSingleError::Unknown],
                ))
            }
        }
    }

    /// Returns a value indicating if the given binary is accepted by this filter expression.
    ///
    /// The value is:
    /// * `Some(true)` if this binary is definitely accepted by this filter expression.
    /// * `Some(false)` if this binary is definitely not accepted.
    /// * `None` if this binary might or might not be accepted.
    pub fn matches_binary(&self, query: &BinaryQuery<'_>) -> Option<bool> {
        use ExprLayer::*;
        self.collapse_layers(|layer: ExprLayer<&FilteringSet, Option<bool>>| {
            match layer {
                Set(set) => set.matches_binary(query),
                Not(a) => a.logic_not(),
                // TODO: or_else/and_then?
                Union(a, b) => a.logic_or(b),
                Intersection(a, b) => a.logic_and(b),
            }
        })
    }

    /// Returns true if the given test is accepted by this filter expression.
    pub fn matches_test(&self, query: &TestQuery<'_>) -> bool {
        use ExprLayer::*;
        self.collapse_layers(|layer: ExprLayer<&FilteringSet, bool>| match layer {
            Set(set) => set.matches_test(query),
            Not(a) => !a,
            Union(a, b) => a || b,
            Intersection(a, b) => a && b,
        })
    }

    /// Returns true if the given expression needs dependencies information to work
    pub fn needs_deps(raw_expr: &str) -> bool {
        // the expression needs dependencies expression if it uses deps(..) or rdeps(..)
        raw_expr.contains("deps")
    }
}

/// A propositional logic used to evaluate `Expression` instances.
///
/// An `Expression` consists of some predicates and the `any`, `all` and `not` operators. An
/// implementation of `Logic` defines how the `any`, `all` and `not` operators should be evaluated.
trait Logic {
    /// The result of an `all` operation with no operands, akin to Boolean `true`.
    fn top() -> Self;

    /// The result of an `any` operation with no operands, akin to Boolean `false`.
    fn bottom() -> Self;

    /// `AND`, which corresponds to the `all` operator.
    fn logic_and(self, other: Self) -> Self;

    /// `OR`, which corresponds to the `any` operator.
    fn logic_or(self, other: Self) -> Self;

    /// `NOT`, which corresponds to the `not` operator.
    fn logic_not(self) -> Self;
}

/// A boolean logic.
impl Logic for bool {
    #[inline]
    fn top() -> Self {
        true
    }

    #[inline]
    fn bottom() -> Self {
        false
    }

    #[inline]
    fn logic_and(self, other: Self) -> Self {
        self && other
    }

    #[inline]
    fn logic_or(self, other: Self) -> Self {
        self || other
    }

    #[inline]
    fn logic_not(self) -> Self {
        !self
    }
}

/// A three-valued logic -- `None` stands for the value being unknown.
///
/// The truth tables for this logic are described on
/// [Wikipedia](https://en.wikipedia.org/wiki/Three-valued_logic#Kleene_and_Priest_logics).
impl Logic for Option<bool> {
    #[inline]
    fn top() -> Self {
        Some(true)
    }

    #[inline]
    fn bottom() -> Self {
        Some(false)
    }

    #[inline]
    fn logic_and(self, other: Self) -> Self {
        match (self, other) {
            // If either is false, the expression is false.
            (Some(false), _) | (_, Some(false)) => Some(false),
            // If both are true, the expression is true.
            (Some(true), Some(true)) => Some(true),
            // One or both are unknown -- the result is unknown.
            _ => None,
        }
    }

    #[inline]
    fn logic_or(self, other: Self) -> Self {
        match (self, other) {
            // If either is true, the expression is true.
            (Some(true), _) | (_, Some(true)) => Some(true),
            // If both are false, the expression is false.
            (Some(false), Some(false)) => Some(false),
            // One or both are unknown -- the result is unknown.
            _ => None,
        }
    }

    #[inline]
    fn logic_not(self) -> Self {
        self.map(|v| !v)
    }
}

/// Haskell madness here:
pub enum ExprLayer<Set, A> {
    Not(A),
    Union(A, A),
    Intersection(A, A),
    Set(Set),
}

impl<A, Set, B> MapLayer<B> for ExprLayer<Set, A> {
    type Unwrapped = A;

    type To = ExprLayer<Set, B>;

    #[inline(always)]
    fn map_layer<F: FnMut(Self::Unwrapped) -> B>(self, mut f: F) -> Self::To {
        use ExprLayer::*;
        match self {
            Not(a) => Not(f(a)),
            Union(a, b) => Union(f(a), f(b)),
            Intersection(a, b) => Intersection(f(a), f(b)),
            Set(f) => Set(f),
        }
    }
}

impl<'a> Project for &'a FilteringExpr {
    type To = ExprLayer<&'a FilteringSet, &'a FilteringExpr>;

    fn project(self) -> Self::To {
        match self {
            FilteringExpr::Not(a) => ExprLayer::Not(a.as_ref()),
            FilteringExpr::Union(a, b) => ExprLayer::Union(a.as_ref(), b.as_ref()),
            FilteringExpr::Intersection(a, b) => ExprLayer::Intersection(a.as_ref(), b.as_ref()),
            FilteringExpr::Set(f) => ExprLayer::Set(f),
        }
    }
}

impl<'a> Project for &'a Expr {
    type To = ExprLayer<&'a SetDef, &'a Expr>;

    fn project(self) -> Self::To {
        match self {
            Expr::Not(a) => ExprLayer::Not(a.as_ref()),
            Expr::Union(a, b) => ExprLayer::Union(a.as_ref(), b.as_ref()),
            Expr::Intersection(a, b) => ExprLayer::Intersection(a.as_ref(), b.as_ref()),
            Expr::Set(f) => ExprLayer::Set(f),
        }
    }
}
