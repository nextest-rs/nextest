// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fmt;

/// An error occurred while parsing a filtering expression
#[derive(Debug)]
pub enum Error {
    // TODO
    /// The parsing failed
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(input) => write!(f, "invalid filter expression: {}", input),
        }
    }
}

impl std::error::Error for Error {}
