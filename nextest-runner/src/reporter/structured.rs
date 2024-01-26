//! Functionality for emitting structured, machine readable output in different
//! formats

mod libtest;
mod recorder;

use super::*;
pub use libtest::{EmitNextestObject, LibtestReporter};
pub use recorder::RecordReporter;

/// Error returned when a user-supplied format version fails to be parsed to a
/// valid and supported version
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

/// A reporter for structured, machine-readable formats.
#[derive(Default)]
pub struct StructuredReporter<'a> {
    /// Libtest-compatible output written to stdout
    libtest: Option<LibtestReporter<'a>>,
    // Recorder for test events.
    record: Option<RecordReporter<'a>>,
}

impl<'a> StructuredReporter<'a> {
    /// Creates a new `StructuredReporter`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets libtest output for the `StructuredReporter`.
    #[inline]
    pub fn set_libtest(&mut self, libtest: LibtestReporter<'a>) -> &mut Self {
        self.libtest = Some(libtest);
        self
    }

    /// Sets recorder output for the `StructuredReporter`.
    #[inline]
    pub fn set_record(&mut self, record: RecordReporter<'a>) -> &mut Self {
        self.record = Some(record);
        self
    }

    #[inline]
    pub(super) fn write_meta(
        &mut self,
        cargo_metadata_json: &Arc<String>,
        test_list: &TestList<'_>,
    ) {
        if let Some(internal) = &mut self.record {
            let cargo_metadata_json = cargo_metadata_json.clone();
            let test_list = test_list.to_summary();
            internal.write_meta(cargo_metadata_json, test_list);
        }
    }

    #[inline]
    pub(super) fn write_event(&mut self, event: TestEvent<'a>) -> Result<(), WriteEventError> {
        if let Some(libtest) = &mut self.libtest {
            libtest.write_event(&event)?;
        }
        if let Some(record) = &mut self.record {
            record.write_event(event);
        }
        Ok(())
    }

    #[inline]
    pub(super) fn finish(self) -> Result<(), WriteEventError> {
        if let Some(internal) = self.record {
            internal.finish();
        }
        Ok(())
    }
}
