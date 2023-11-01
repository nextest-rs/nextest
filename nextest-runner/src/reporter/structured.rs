//! Functionality for emitting structured, machine readable output in different
//! formats

mod libtest;

use super::*;
pub use libtest::{EmitNextestObject, LibtestReporter};

/// Error returned when a user-supplied format version fails to be parsed to a
/// valid and supported version
///
/// This
#[derive(Clone, Debug, thiserror::Error)]
#[error("invalid format version: {input}")]
pub struct FormatVersionError {
    /// The input that failed to parse.
    pub input: String,
    /// The underlying error
    #[source]
    pub err: FormatVersionErrorInner,
}

/// The different errors that can occur when parsing and validating a format version
#[derive(Clone, Debug, thiserror::Error)]
pub enum FormatVersionErrorInner {
    /// The input did not have a valid syntax
    #[error("expected format version in form of `{expected}`")]
    InvalidFormat {
        /// The expected pseudo format
        expected: &'static str,
    },
    /// A decimal integer was expected but could not be parsed
    #[error("version component `{which}` could not be parsed as an integer")]
    InvalidInteger {
        /// Which component was invalid
        which: &'static str,
        /// The parse failure
        #[source]
        err: std::num::ParseIntError,
    },
    /// The version component was not within th expected range
    #[error("version component `{which}` value {value} is out of range {range:?}")]
    InvalidValue {
        /// The component which was out of range
        which: &'static str,
        /// The value that was parsed
        value: u8,
        /// The range of valid values for the component
        range: std::ops::Range<u8>,
    },
}

/// A reporter for structured, machine readable, output based on the user's
/// preference
pub enum StructuredReporter<'a> {
    /// Libtest compatible output
    Libtest(LibtestReporter<'a>),
    // TODO: make a custom format that is easier to consume than libtest's
    //Json(json::JsonReporter<'a>),
    /// Variant that doesn't actually emit anything
    Disabled,
}

impl<'a> StructuredReporter<'a> {
    #[inline]
    pub(super) fn write_event(&mut self, event: &TestEvent<'a>) -> Result<(), WriteEventError> {
        match self {
            Self::Disabled => Ok(()),
            Self::Libtest(ltr) => ltr.write_event(event),
        }
    }
}
