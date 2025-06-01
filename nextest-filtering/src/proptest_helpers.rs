// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    NameMatcher,
    parsing::{
        AndOperator, DifferenceOperator, GenericGlob, NotOperator, OrOperator, ParsedExpr,
        ParsedLeaf,
    },
};
use guppy::graph::cargo::BuildPlatform;
use proptest::prelude::*;

impl ParsedExpr<()> {
    #[doc(hidden)]
    pub fn strategy() -> impl Strategy<Value = Self> {
        let leaf = ParsedLeaf::strategy().prop_map(Self::Set);

        leaf.prop_recursive(8, 256, 10, |inner| {
            // Since `Expr` explicitly tracks parentheses, the below blocks need to add parentheses
            // in places where they'd be necessary to parse out. For example, if the original
            // expression is:
            //
            // Not(And("foo", "bar"))
            //
            // then it will be printed out as:
            //
            // not foo and bar
            //
            // which will parse as:
            //
            // And(Not("foo"), "bar")
            //
            // Adding parentheses in the right locations prevents these sorts of cases from being
            // generated.
            prop_oneof![
                1 => (any::<NotOperator>(), inner.clone()).prop_map(|(op, a)| {
                    // Add parens to any other operations inside a not operator.
                    Self::Not(op, a.parenthesize_not())
                }),
                1 => (any::<OrOperator>(), inner.clone(), inner.clone()).prop_map(|(op, a, b)| {
                    Self::Union(op, a.parenthesize_or_left(), b.parenthesize_or_right())
                }),
                1 => (any::<AndOperator>(), inner.clone(), inner.clone()).prop_map(|(op, a, b)| {
                    // Add parens to an or operation inside an and operation.
                    Self::Intersection(op, a.parenthesize_and_left(), b.parenthesize_and_right())
                }),
                1 => (any::<DifferenceOperator>(), inner.clone(), inner.clone()).prop_map(|(op, a, b)| {
                    Self::Difference(op, a.parenthesize_and_left(), b.parenthesize_and_right())
                }),
                1 => inner.prop_map(|a| Self::Parens(Box::new(a))),
            ]
        })
    }

    /// Adds parens to any other operations inside a not operator.
    fn parenthesize_not(self) -> Box<Self> {
        match &self {
            Self::Union(_, _, _) | Self::Intersection(_, _, _) | Self::Difference(_, _, _) => {
                Box::new(Self::Parens(Box::new(self)))
            }
            Self::Set(_) | Self::Not(_, _) | Self::Parens(_) => Box::new(self),
        }
    }

    /// This is currently a no-op.
    fn parenthesize_or_left(self) -> Box<Self> {
        Box::new(self)
    }

    /// Adds parens to an or operation inside the right side of an or operation.
    fn parenthesize_or_right(self) -> Box<Self> {
        match &self {
            Self::Union(_, _, _) => Box::new(Self::Parens(Box::new(self))),
            Self::Set(_)
            | Self::Intersection(_, _, _)
            | Self::Difference(_, _, _)
            | Self::Not(_, _)
            | Self::Parens(_) => Box::new(self),
        }
    }

    /// Adds parens to an or operation inside the left side of an and or difference operation.
    fn parenthesize_and_left(self) -> Box<Self> {
        match &self {
            Self::Union(_, _, _) => Box::new(Self::Parens(Box::new(self))),
            Self::Set(_)
            | Self::Intersection(_, _, _)
            | Self::Difference(_, _, _)
            | Self::Not(_, _)
            | Self::Parens(_) => Box::new(self),
        }
    }

    /// Adds parens to an or, and, or difference operation inside the right side of an and
    /// or difference operation.
    fn parenthesize_and_right(self) -> Box<Self> {
        match &self {
            Self::Union(_, _, _) | Self::Intersection(_, _, _) | Self::Difference(_, _, _) => {
                Box::new(Self::Parens(Box::new(self)))
            }
            Self::Set(_) | Self::Not(_, _) | Self::Parens(_) => Box::new(self),
        }
    }
}

impl ParsedLeaf<()> {
    pub(crate) fn strategy() -> impl Strategy<Value = Self> {
        prop_oneof![
            1 => NameMatcher::default_glob_strategy().prop_map(|s| Self::Package(s, ())),
            1 => NameMatcher::default_glob_strategy().prop_map(|s| Self::Deps(s, ())),
            1 => NameMatcher::default_glob_strategy().prop_map(|s| Self::Rdeps(s, ())),
            1 => NameMatcher::default_equal_strategy().prop_map(|s| Self::Kind(s, ())),
            1 => NameMatcher::default_glob_strategy().prop_map(|s| Self::Binary(s, ())),
            1 => NameMatcher::default_glob_strategy().prop_map(|s| Self::BinaryId(s, ())),
            1 => build_platform_strategy().prop_map(|p| Self::Platform(p, ())),
            1 => NameMatcher::default_contains_strategy().prop_map(|s| Self::Test(s, ())),
            1 => Just(Self::All),
            1 => Just(Self::None),
        ]
    }
}

impl NameMatcher {
    pub(crate) fn default_equal_strategy() -> impl Strategy<Value = Self> {
        prop_oneof![
            1 => (name_strategy(), any::<bool>()).prop_filter_map(
                "implicit = true can't begin with operators",
                |(value, implicit)| {
                    let accept = match (implicit, begins_with_operator(&value)) {
                        (false, _) => true,
                        (true, false) => true,
                        (true, true) => false,
                    };
                    accept.then_some(Self::Equal { value, implicit })
                },
            ),
            1 => name_strategy().prop_map(|value| {
                Self::Contains { value, implicit: false }
            }),
            1 => regex_strategy().prop_map(Self::Regex),
            1 => glob_strategy().prop_map(|glob| { Self::Glob { glob, implicit: false }}),
        ]
    }

    pub(crate) fn default_contains_strategy() -> impl Strategy<Value = Self> {
        prop_oneof![
            1 => name_strategy().prop_map(|value| {
                Self::Equal { value, implicit: false }
            }),
            1 => (name_strategy(), any::<bool>()).prop_filter_map(
                "implicit = true can't begin with operators",
                |(value, implicit)| {
                    let accept = match (implicit, begins_with_operator(&value)) {
                        (false, _) => true,
                        (true, false) => true,
                        (true, true) => false,
                    };
                    accept.then_some(Self::Contains { value, implicit })
            }),
            1 => regex_strategy().prop_map(Self::Regex),
            1 => glob_strategy().prop_map(|glob| { Self::Glob { glob, implicit: false }}),
        ]
    }

    pub(crate) fn default_glob_strategy() -> impl Strategy<Value = Self> {
        prop_oneof![
            1 => name_strategy().prop_map(|value| {
                Self::Equal { value, implicit: false }
            }),
            1 => name_strategy().prop_map(|value| {
                Self::Contains { value, implicit: false }
            }),
            1 => regex_strategy().prop_map(Self::Regex),
            1 => (glob_strategy(), any::<bool>()).prop_filter_map(
                "implicit = true can't begin with operators",
                |(glob, implicit)| {
                    let accept = match (implicit, begins_with_operator(glob.as_str())) {
                        (false, _) => true,
                        (true, false) => true,
                        (true, true) => false,
                    };
                    accept.then_some(Self::Glob { glob, implicit })
                },
            ),
        ]
    }
}

fn begins_with_operator(value: &str) -> bool {
    value.starts_with('=')
        || value.starts_with('~')
        || value.starts_with('/')
        || value.starts_with('#')
}

pub(crate) fn build_platform_strategy() -> impl Strategy<Value = BuildPlatform> {
    prop::sample::select(&[BuildPlatform::Host, BuildPlatform::Target][..])
}

pub(crate) fn name_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[abcde]{0,10}",
        // Some escapes and glob characters
        1 => r"[abcde=/~#*?]{0,10}",
        // More escapes
        1 => r"[abcde=/~#*?\[\]\r\t\n\u{2055}\u{1fe4e}]{0,10}",
    ]
}

pub(crate) fn glob_strategy() -> impl Strategy<Value = GenericGlob> {
    glob_str_strategy().prop_filter_map(
        "some strings generated by the strategy are invalid globs",
        |s| GenericGlob::new(s).ok(),
    )
}

fn glob_str_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // No escapes or glob characters.
        4 => "[abcde]{0,10}",
        // Some escapes and glob characters
        1 => r"[abcde*?\[\]]{0,10}",
        // More escapes
        1 => r"[abcde=/~#*?\[\]\r\t\n\u{2055}\u{1fe4e}]{0,10}",
    ]
}

pub(crate) fn regex_strategy() -> impl Strategy<Value = regex::Regex> {
    regex_str_strategy().prop_map(|s| {
        regex::Regex::new(&s).expect("all regexes generated by the strategy are valid")
    })
}

fn regex_str_strategy() -> impl Strategy<Value = String> {
    // TODO: add more cases here
    let leaf = prop_oneof![
        4 => "[abcde]{0,10}",
        // Some escapes
        1 => r"([abcde]|(\\\?)|(\\\*)|){0,10}",
        // More escapes
        1 => r"[abcde/\r\t\n\u{2055}\u{1fe4e}]{0,10}",
    ];

    // TODO: add more recursion cases here
    leaf.prop_recursive(
        // Up to 3 levels deep
        3,
        // Max size 16 nodes
        16,
        // Up to 3 items per collection
        3,
        |inner| {
            prop_oneof![
                1 => (inner.clone(), inner.clone()).prop_map(|(a, b)| {
                    format!("{a}{b}")
                }),
                1 => (inner.clone(), inner.clone()).prop_map(|(a, b)| {
                    format!("({a})|({b})")
                }),
                1 => inner.clone().prop_map(|a| {
                    format!("({a})*")
                }),
                1 => inner.prop_map(|a| {
                    format!("({a})?")
                }),
            ]
        },
    )
}

// > instance Arbitrary (RegExp Char) where
// >   arbitrary = sized regexp
// >
// > regexp :: Int -> Gen (RegExp Char)
// > regexp 0 = frequency [ (1, return eps)
// >                      , (4, char `fmap` simpleChar) ]
// > regexp n = frequency [ (3, regexp 0)
// >                      , (1, alt  `fmap` subexp `ap` subexp)
// >                      , (2, seq_ `fmap` subexp `ap` subexp)
// >                      , (1, rep  `fmap` regexp (n-1))
// >                      , (2, fromString `fmap` parsedRegExp n) ]
// >  where subexp = regexp (n `div` 2)
// >
// > simpleChar :: Gen Char
// > simpleChar = elements "abcde"
// >
// > parsedRegExp :: Int -> Gen String
// > parsedRegExp n = frequency [ (4, symClass)
// >                            , (2, (++"?") `fmap` subexp)
// >                            , (2, (++"+") `fmap` subexp)
// >                            , (1, mkBrep1 =<< subexp)
// >                            , (1, mkBrep2 =<< subexp) ]
// >  where
// >   subexp = (($"") . showParen True . shows)
// >     `fmap` (resize (n-1) arbitrary :: Gen (RegExp Char))
// >
// >   mkBrep1 r = do x <- elements [0..3] :: Gen Int
// >                  return $ r ++ "{" ++ show x ++ "}"
// >
// >   mkBrep2 r = do x <- elements [0..2] :: Gen Int
// >                  y <- elements [0..2] :: Gen Int
// >                  return $ r ++ "{" ++ show x ++ "," ++ show (x+y) ++ "}"
// >
// > symClass :: Gen String
// > symClass = frequency [ (1, specialChar)
// >                      , (2, do n <- choose (0,3)
// >                               cs <- replicateM n charClass
// >                               s <- (["","^"]!!) `fmap` choose (0,1)
// >                               return $ "[" ++ s ++ concat cs ++ "]") ]
// >  where
// >   specialChar = elements (map (:[]) "." ++
// >                           map (\c -> '\\':[c]) "abcdewWdDsS \\|*+?.[]{}^")
// >   charClass   = oneof [ (:[]) `fmap` simpleChar
// >                       , specialChar
// >                       , do x <- simpleChar
// >                            y <- simpleChar
// >                            return $ x : '-' : [chr (ord x+ord y-ord 'a')] ]

#[cfg(test)]
mod tests {
    use super::*;

    #[test_strategy::proptest]
    fn proptest_regex_valid(#[strategy(regex_str_strategy())] regex_str: String) {
        println!("regex_str = {regex_str:?}");
        regex::Regex::new(&regex_str).expect("all regexes generated by the strategy are valid");
    }
}
