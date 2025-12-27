// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::junit::MetadataJunit;
use crate::{
    config::core::EvaluatableProfile, errors::WriteEventError, reporter::events::TestEvent,
    run_mode::NextestRunMode,
};
use camino::Utf8PathBuf;

#[derive(Clone, Debug)]
#[expect(dead_code)]
pub(crate) struct EventAggregator<'cfg> {
    store_dir: Utf8PathBuf,
    // TODO: log information in a JSONable report (converting that to XML later) instead of directly
    // writing it to XML
    junit: Option<MetadataJunit<'cfg>>,
}

impl<'cfg> EventAggregator<'cfg> {
    pub(crate) fn new(mode: NextestRunMode, profile: &EvaluatableProfile<'cfg>) -> Self {
        Self {
            store_dir: profile.store_dir().to_owned(),
            junit: profile.junit().map(|cfg| MetadataJunit::new(mode, cfg)),
        }
    }

    pub(crate) fn write_event(
        &mut self,
        event: Box<TestEvent<'cfg>>,
    ) -> Result<(), WriteEventError> {
        if let Some(junit) = &mut self.junit {
            junit.write_event(event)?;
        }
        Ok(())
    }
}
