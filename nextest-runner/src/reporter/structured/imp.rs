// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Functionality for emitting structured, machine readable output in different
//! formats

use super::LibtestReporter;
use crate::{errors::WriteEventError, reporter::events::TestEvent};

/// A reporter for structured, machine-readable formats.
#[derive(Default)]
pub struct StructuredReporter<'a> {
    /// Libtest-compatible output written to stdout
    libtest: Option<LibtestReporter<'a>>,
    // Internal structured reporter.
    // internal: Option<T>,
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

    #[inline]
    pub(crate) fn write_event(&mut self, event: &TestEvent<'a>) -> Result<(), WriteEventError> {
        if let Some(libtest) = &mut self.libtest {
            libtest.write_event(event)?;
        }
        Ok(())
    }
}
