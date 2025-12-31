// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::{FiltersetParseErrors, ParseSingleError},
    parsing::{
        DisplayParsedRegex, DisplayParsedString, ExprResult, GenericGlob, ParsedExpr, ParsedLeaf,
        new_span, parse,
    },
};
use guppy::{
    PackageId,
    graph::{BuildTargetId, PackageGraph, PackageMetadata, cargo::BuildPlatform},
};
use miette::SourceSpan;
use nextest_metadata::{RustBinaryId, RustTestBinaryKind, TestCaseName};
use recursion::{Collapsible, CollapsibleExt, MappableFrame, PartiallyApplied};
use smol_str::SmolStr;
use std::{collections::HashSet, fmt, sync::OnceLock};

/// Matcher for name
///
/// Used both for package name and test name
#[derive(Debug, Clone)]
pub enum NameMatcher {
    /// Exact value
    Equal { value: String, implicit: bool },
    /// Simple contains test
    Contains { value: String, implicit: bool },
    /// Test against a glob
    Glob { glob: GenericGlob, implicit: bool },
    /// Test against a regex
    Regex(regex::Regex),
}

impl NameMatcher {
    pub(crate) fn implicit_equal(value: String) -> Self {
        Self::Equal {
            value,
            implicit: true,
        }
    }

    pub(crate) fn implicit_contains(value: String) -> Self {
        Self::Contains {
            value,
            implicit: true,
        }
    }
}

impl PartialEq for NameMatcher {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Contains {
                    value: s1,
                    implicit: default1,
                },
                Self::Contains {
                    value: s2,
                    implicit: default2,
                },
            ) => s1 == s2 && default1 == default2,
            (
                Self::Equal {
                    value: s1,
                    implicit: default1,
                },
                Self::Equal {
                    value: s2,
                    implicit: default2,
                },
            ) => s1 == s2 && default1 == default2,
            (Self::Regex(r1), Self::Regex(r2)) => r1.as_str() == r2.as_str(),
            (Self::Glob { glob: g1, .. }, Self::Glob { glob: g2, .. }) => {
                g1.regex().as_str() == g2.regex().as_str()
            }
            _ => false,
        }
    }
}

impl Eq for NameMatcher {}

impl fmt::Display for NameMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Equal { value, implicit } => write!(
                f,
                "{}{}",
                if *implicit { "" } else { "=" },
                DisplayParsedString(value)
            ),
            Self::Contains { value, implicit } => write!(
                f,
                "{}{}",
                if *implicit { "" } else { "~" },
                DisplayParsedString(value)
            ),
            Self::Glob { glob, implicit } => write!(
                f,
                "{}{}",
                if *implicit { "" } else { "#" },
                DisplayParsedString(glob.as_str())
            ),
            Self::Regex(r) => write!(f, "/{}/", DisplayParsedRegex(r)),
        }
    }
}

/// A leaf node in a filterset expression tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FiltersetLeaf {
    /// All tests in packages
    Packages(HashSet<PackageId>),
    /// All tests present in this kind of binary.
    Kind(NameMatcher, SourceSpan),
    /// The platform a test is built for.
    Platform(BuildPlatform, SourceSpan),
    /// All binaries matching a name
    Binary(NameMatcher, SourceSpan),
    /// All binary IDs matching a name
    BinaryId(NameMatcher, SourceSpan),
    /// All tests matching a name
    Test(NameMatcher, SourceSpan),
    /// The default set of tests to run.
    Default,
    /// All tests
    All,
    /// No tests
    None,
}

impl FiltersetLeaf {
    /// Returns true if this leaf can only be evaluated at runtime, i.e. it
    /// requires test names to be available.
    ///
    /// Currently, this also returns true (conservatively) for the `Default`
    /// leaf, which is used to represent the default set of tests to run.
    pub fn is_runtime_only(&self) -> bool {
        matches!(self, Self::Test(_, _) | Self::Default)
    }
}

/// A query for a binary, passed into [`Filterset::matches_binary`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct BinaryQuery<'a> {
    /// The package ID.
    pub package_id: &'a PackageId,

    /// The binary ID.
    pub binary_id: &'a RustBinaryId,

    /// The name of the binary.
    pub binary_name: &'a str,

    /// The kind of binary this test is (lib, test etc).
    pub kind: &'a RustTestBinaryKind,

    /// The platform this test is built for.
    pub platform: BuildPlatform,
}

/// A query for a specific test, passed into [`Filterset::matches_test`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TestQuery<'a> {
    /// The binary query.
    pub binary_query: BinaryQuery<'a>,

    /// The name of the test.
    pub test_name: &'a TestCaseName,
}

/// A filterset that has been parsed and compiled.
///
/// Used to filter tests to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filterset {
    /// The raw expression passed in.
    pub input: String,

    /// The parsed-but-not-compiled expression.
    pub parsed: ParsedExpr,

    /// The compiled expression.
    pub compiled: CompiledExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompiledExpr {
    /// Accepts every test not in the given expression
    Not(Box<CompiledExpr>),
    /// Accepts every test in either given expression
    Union(Box<CompiledExpr>, Box<CompiledExpr>),
    /// Accepts every test in both given expressions
    Intersection(Box<CompiledExpr>, Box<CompiledExpr>),
    /// Accepts every test in a set
    Set(FiltersetLeaf),
}

impl CompiledExpr {
    /// Returns a value indicating all tests are accepted by this filterset.
    pub const ALL: Self = CompiledExpr::Set(FiltersetLeaf::All);

    /// Returns a value indicating if the given binary is accepted by this filterset.
    ///
    /// The value is:
    /// * `Some(true)` if this binary is definitely accepted by this filterset.
    /// * `Some(false)` if this binary is definitely not accepted.
    /// * `None` if this binary might or might not be accepted.
    pub fn matches_binary(&self, query: &BinaryQuery<'_>, cx: &EvalContext<'_>) -> Option<bool> {
        use ExprFrame::*;
        Wrapped(self).collapse_frames(|layer: ExprFrame<&FiltersetLeaf, Option<bool>>| {
            match layer {
                Set(set) => set.matches_binary(query, cx),
                Not(a) => a.logic_not(),
                // TODO: or_else/and_then?
                Union(a, b) => a.logic_or(b),
                Intersection(a, b) => a.logic_and(b),
                Difference(a, b) => a.logic_and(b.logic_not()),
                Parens(a) => a,
            }
        })
    }

    /// Returns true if the given test is accepted by this filterset.
    pub fn matches_test(&self, query: &TestQuery<'_>, cx: &EvalContext<'_>) -> bool {
        use ExprFrame::*;
        Wrapped(self).collapse_frames(|layer: ExprFrame<&FiltersetLeaf, bool>| match layer {
            Set(set) => set.matches_test(query, cx),
            Not(a) => !a,
            Union(a, b) => a || b,
            Intersection(a, b) => a && b,
            Difference(a, b) => a && !b,
            Parens(a) => a,
        })
    }
}

impl NameMatcher {
    pub(crate) fn is_match(&self, input: &str) -> bool {
        match self {
            Self::Equal { value, .. } => value == input,
            Self::Contains { value, .. } => input.contains(value),
            Self::Glob { glob, .. } => glob.is_match(input),
            Self::Regex(reg) => reg.is_match(input),
        }
    }
}

impl FiltersetLeaf {
    fn matches_test(&self, query: &TestQuery<'_>, cx: &EvalContext) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Default => cx.default_filter.matches_test(query, cx),
            Self::Test(matcher, _) => matcher.is_match(query.test_name.as_str()),
            Self::Binary(matcher, _) => matcher.is_match(query.binary_query.binary_name),
            Self::BinaryId(matcher, _) => matcher.is_match(query.binary_query.binary_id.as_str()),
            Self::Platform(platform, _) => query.binary_query.platform == *platform,
            Self::Kind(matcher, _) => matcher.is_match(query.binary_query.kind.as_str()),
            Self::Packages(packages) => packages.contains(query.binary_query.package_id),
        }
    }

    fn matches_binary(&self, query: &BinaryQuery<'_>, cx: &EvalContext) -> Option<bool> {
        match self {
            Self::All => Logic::top(),
            Self::None => Logic::bottom(),
            Self::Default => cx.default_filter.matches_binary(query, cx),
            Self::Test(_, _) => None,
            Self::Binary(matcher, _) => Some(matcher.is_match(query.binary_name)),
            Self::BinaryId(matcher, _) => Some(matcher.is_match(query.binary_id.as_str())),
            Self::Platform(platform, _) => Some(query.platform == *platform),
            Self::Kind(matcher, _) => Some(matcher.is_match(query.kind.as_str())),
            Self::Packages(packages) => Some(packages.contains(query.package_id)),
        }
    }
}

/// Inputs to filterset parsing.
#[derive(Debug)]
pub struct ParseContext<'g> {
    /// The package graph.
    graph: &'g PackageGraph,

    /// Cached data computed on first access.
    cache: OnceLock<ParseContextCache<'g>>,
}

impl<'g> ParseContext<'g> {
    /// Creates a new `ParseContext`.
    #[inline]
    pub fn new(graph: &'g PackageGraph) -> Self {
        Self {
            graph,
            cache: OnceLock::new(),
        }
    }

    /// Returns the package graph.
    #[inline]
    pub fn graph(&self) -> &'g PackageGraph {
        self.graph
    }

    pub(crate) fn make_cache(&self) -> &ParseContextCache<'g> {
        self.cache
            .get_or_init(|| ParseContextCache::new(self.graph))
    }
}

#[derive(Debug)]
pub(crate) struct ParseContextCache<'g> {
    pub(crate) workspace_packages: Vec<PackageMetadata<'g>>,
    // Ordinarily we'd store RustBinaryId here, but that wouldn't allow looking
    // up a string.
    pub(crate) binary_ids: HashSet<SmolStr>,
    pub(crate) binary_names: HashSet<&'g str>,
}

impl<'g> ParseContextCache<'g> {
    fn new(graph: &'g PackageGraph) -> Self {
        let workspace_packages: Vec<_> = graph
            .resolve_workspace()
            .packages(guppy::graph::DependencyDirection::Forward)
            .collect();
        let (binary_ids, binary_names) = workspace_packages
            .iter()
            .flat_map(|pkg| {
                pkg.build_targets().filter_map(|bt| {
                    let kind = compute_kind(&bt.id())?;
                    let binary_id = RustBinaryId::from_parts(pkg.name(), &kind, bt.name());
                    Some((SmolStr::new(binary_id.as_str()), bt.name()))
                })
            })
            .unzip();

        Self {
            workspace_packages,
            binary_ids,
            binary_names,
        }
    }
}

fn compute_kind(id: &BuildTargetId<'_>) -> Option<RustTestBinaryKind> {
    match id {
        // Note this covers both libraries and proc macros, but we treat
        // libraries the same as proc macros while constructing a `RustBinaryId`
        // anyway.
        BuildTargetId::Library => Some(RustTestBinaryKind::LIB),
        BuildTargetId::Benchmark(_) => Some(RustTestBinaryKind::BENCH),
        BuildTargetId::Example(_) => Some(RustTestBinaryKind::EXAMPLE),
        BuildTargetId::BuildScript => {
            // Build scripts don't have tests in them.
            None
        }
        BuildTargetId::Binary(_) => Some(RustTestBinaryKind::BIN),
        BuildTargetId::Test(_) => Some(RustTestBinaryKind::TEST),
        _ => panic!("unknown build target id: {id:?}"),
    }
}

/// The kind of filterset being parsed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FiltersetKind {
    /// A test filterset.
    Test,

    /// A test archive filterset.
    TestArchive,

    /// A default-filter filterset.
    ///
    /// To prevent recursion, default-filter expressions cannot contain `default()` themselves.
    /// (This is a limited kind of the infinite recursion checking we'll need to do in the future.)
    DefaultFilter,
}

impl fmt::Display for FiltersetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Test => write!(f, "test"),
            Self::TestArchive => write!(f, "archive-filter"),
            Self::DefaultFilter => write!(f, "default-filter"),
        }
    }
}

/// Inputs to filterset evaluation functions.
#[derive(Copy, Clone, Debug)]
pub struct EvalContext<'a> {
    /// The default set of tests to run.
    pub default_filter: &'a CompiledExpr,
}

impl Filterset {
    /// Parse a filterset.
    pub fn parse(
        input: String,
        cx: &ParseContext<'_>,
        kind: FiltersetKind,
    ) -> Result<Self, FiltersetParseErrors> {
        let mut errors = Vec::new();
        match parse(new_span(&input, &mut errors)) {
            Ok(parsed_expr) => {
                if !errors.is_empty() {
                    return Err(FiltersetParseErrors::new(input.clone(), errors));
                }

                match parsed_expr {
                    ExprResult::Valid(parsed) => {
                        let compiled = crate::compile::compile(&parsed, cx, kind)
                            .map_err(|errors| FiltersetParseErrors::new(input.clone(), errors))?;
                        Ok(Self {
                            input,
                            parsed,
                            compiled,
                        })
                    }
                    _ => {
                        // should not happen
                        // If an ParsedExpr::Error is produced, we should also have an error inside
                        // errors and we should already have returned
                        // IMPROVE this is an internal error => add log to suggest opening an bug ?
                        Err(FiltersetParseErrors::new(
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
                Err(FiltersetParseErrors::new(
                    input,
                    vec![ParseSingleError::Unknown],
                ))
            }
        }
    }

    /// Returns a value indicating if the given binary is accepted by this filterset.
    ///
    /// The value is:
    /// * `Some(true)` if this binary is definitely accepted by this filterset.
    /// * `Some(false)` if this binary is definitely not accepted.
    /// * `None` if this binary might or might not be accepted.
    pub fn matches_binary(&self, query: &BinaryQuery<'_>, cx: &EvalContext<'_>) -> Option<bool> {
        self.compiled.matches_binary(query, cx)
    }

    /// Returns true if the given test is accepted by this filterset.
    pub fn matches_test(&self, query: &TestQuery<'_>, cx: &EvalContext<'_>) -> bool {
        self.compiled.matches_test(query, cx)
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

pub(crate) enum ExprFrame<Set, A> {
    Not(A),
    Union(A, A),
    Intersection(A, A),
    Difference(A, A),
    Parens(A),
    Set(Set),
}

impl<Set> MappableFrame for ExprFrame<Set, PartiallyApplied> {
    type Frame<Next> = ExprFrame<Set, Next>;

    fn map_frame<A, B>(input: Self::Frame<A>, mut f: impl FnMut(A) -> B) -> Self::Frame<B> {
        use ExprFrame::*;
        match input {
            Not(a) => Not(f(a)),
            // Note: reverse the order because the recursion crate processes
            // entries via a stack, as LIFO. Calling f(b) before f(a) means
            // error messages for a show up before those for b.
            Union(a, b) => {
                let b = f(b);
                let a = f(a);
                Union(a, b)
            }
            Intersection(a, b) => {
                let b = f(b);
                let a = f(a);
                Intersection(a, b)
            }
            Difference(a, b) => {
                let b = f(b);
                let a = f(a);
                Difference(a, b)
            }
            Parens(a) => Parens(f(a)),
            Set(f) => Set(f),
        }
    }
}

// Wrapped struct to prevent trait impl leakages.
pub(crate) struct Wrapped<T>(pub(crate) T);

impl<'a> Collapsible for Wrapped<&'a CompiledExpr> {
    type FrameToken = ExprFrame<&'a FiltersetLeaf, PartiallyApplied>;

    fn into_frame(self) -> <Self::FrameToken as MappableFrame>::Frame<Self> {
        match self.0 {
            CompiledExpr::Not(a) => ExprFrame::Not(Wrapped(a.as_ref())),
            CompiledExpr::Union(a, b) => ExprFrame::Union(Wrapped(a.as_ref()), Wrapped(b.as_ref())),
            CompiledExpr::Intersection(a, b) => {
                ExprFrame::Intersection(Wrapped(a.as_ref()), Wrapped(b.as_ref()))
            }
            CompiledExpr::Set(f) => ExprFrame::Set(f),
        }
    }
}

impl<'a> Collapsible for Wrapped<&'a ParsedExpr> {
    type FrameToken = ExprFrame<&'a ParsedLeaf, PartiallyApplied>;

    fn into_frame(self) -> <Self::FrameToken as MappableFrame>::Frame<Self> {
        match self.0 {
            ParsedExpr::Not(_, a) => ExprFrame::Not(Wrapped(a.as_ref())),
            ParsedExpr::Union(_, a, b) => {
                ExprFrame::Union(Wrapped(a.as_ref()), Wrapped(b.as_ref()))
            }
            ParsedExpr::Intersection(_, a, b) => {
                ExprFrame::Intersection(Wrapped(a.as_ref()), Wrapped(b.as_ref()))
            }
            ParsedExpr::Difference(_, a, b) => {
                ExprFrame::Difference(Wrapped(a.as_ref()), Wrapped(b.as_ref()))
            }
            ParsedExpr::Parens(a) => ExprFrame::Parens(Wrapped(a.as_ref())),
            ParsedExpr::Set(f) => ExprFrame::Set(f),
        }
    }
}
