// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Functionality for emitting structured, machine readable output in different
//! formats.

use super::{LibtestReporter, RecordReporter};
use crate::{
    errors::WriteEventError,
    record::{RecordOpts, StoreSizes},
    reporter::events::TestEvent,
};
use nextest_metadata::TestListSummary;
use std::sync::Arc;

/// A reporter for structured, machine-readable formats.
///
/// This reporter can emit output in multiple formats simultaneously:
/// - Libtest-compatible JSON to stdout.
/// - Recording to disk for later inspection.
#[derive(Default)]
pub struct StructuredReporter<'a> {
    /// Libtest-compatible output written to stdout.
    libtest: Option<LibtestReporter<'a>>,
    /// Recording reporter for writing to disk.
    record: Option<RecordReporter<'a>>,
}

impl<'a> StructuredReporter<'a> {
    /// Creates a new `StructuredReporter`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets libtest output for the `StructuredReporter`.
    pub fn set_libtest(&mut self, libtest: LibtestReporter<'a>) -> &mut Self {
        self.libtest = Some(libtest);
        self
    }

    /// Sets the record reporter for the `StructuredReporter`.
    pub fn set_record(&mut self, record: RecordReporter<'a>) -> &mut Self {
        self.record = Some(record);
        self
    }

    /// Writes metadata to the record reporter, if configured.
    ///
    /// This should be called once at the beginning of a test run.
    pub fn write_meta(
        &self,
        cargo_metadata_json: Arc<String>,
        test_list: TestListSummary,
        opts: RecordOpts,
    ) {
        if let Some(record) = &self.record {
            record.write_meta(cargo_metadata_json, test_list, opts);
        }
    }

    /// Writes a test event to all configured reporters.
    #[inline]
    pub(crate) fn write_event(&mut self, event: &TestEvent<'a>) -> Result<(), WriteEventError> {
        if let Some(libtest) = &mut self.libtest {
            libtest.write_event(event)?;
        }
        if let Some(record) = &self.record {
            // Clone the event for the record reporter since it runs in a separate thread.
            record.write_event(event.clone());
        }
        Ok(())
    }

    /// Finishes writing to all configured reporters.
    ///
    /// Returns the sizes of the recording (compressed and uncompressed), or `None` if recording
    /// was not enabled or an error occurred.
    ///
    /// This should be called at the end of a test run to ensure all data is flushed.
    pub fn finish(self) -> Option<StoreSizes> {
        if let Some(record) = self.record {
            match record.finish() {
                Ok(sizes) => Some(sizes),
                Err(error) => {
                    tracing::error!("error finishing run recording: {error}");
                    None
                }
            }
        } else {
            None
        }
    }
}
