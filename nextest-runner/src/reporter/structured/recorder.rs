// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reporter for recording test runs.

use super::TestEvent;
use crate::{
    errors::RecordReporterError,
    run_store::{InMemoryOutput, RunRecorder, TestEventSummary},
};
use display_error_chain::DisplayErrorChain;
use nextest_metadata::TestListSummary;
use std::{
    sync::{mpsc, Arc},
    thread::JoinHandle,
};

/// A reporter used for recording test runs.
#[derive(Debug)]
pub struct RecordReporter<'a> {
    // Invariant: sender is always Some while the reporter is alive.
    sender: Option<mpsc::SyncSender<RecordEvent>>,
    handle: JoinHandle<()>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> RecordReporter<'a> {
    /// Creates a new `RecordReporter`.
    pub fn new(run_recorder: RunRecorder) -> Self {
        // Spawn a thread to do the writing.
        let (sender, receiver) = mpsc::sync_channel(128);
        let handle = std::thread::spawn(move || {
            let mut writer = RecordReporterWriter { run_recorder };
            while let Ok(event) = receiver.recv() {
                if let Err(error) = writer.handle_event(event) {
                    log::error!(
                        "error recording run, will no longer store events: {}",
                        DisplayErrorChain::new(&error)
                    );
                    return;
                }
            }

            // The sender has been dropped. Finish writing and exit.
            if let Err(error) = writer.finish() {
                log::error!(
                    "error finishing run recording: {}",
                    DisplayErrorChain::new(&error)
                );
            }
        });
        Self {
            sender: Some(sender),
            handle,
            _marker: std::marker::PhantomData,
        }
    }

    /// Writes metadata to the internal reporter.
    pub fn write_meta(&self, cargo_metadata_json: Arc<String>, test_list: TestListSummary) {
        let event = RecordEvent::Meta {
            cargo_metadata_json,
            test_list,
        };
        // Ignore receive errors because they indicate that the receiver has exited (likely a
        // panic, dealt with in finish()).
        _ = self.sender.as_ref().unwrap().send(event);
    }

    /// Writes a test event to the internal reporter.
    pub fn write_event(&self, event: TestEvent<'_>) {
        let event = RecordEvent::TestEvent(TestEventSummary::from_test_event(event));
        // Ignore receive errors because they indicate that the receiver has exited (likely a
        // panic, dealt with in finish()).
        _ = self.sender.as_ref().unwrap().send(event);
    }

    /// Finishes writing to the internal reporter.
    ///
    /// This must be called before the reporter is dropped.
    pub fn finish(mut self) {
        // Drop the sender, which signals the receiver to exit.
        let sender = self.sender.take();
        std::mem::drop(sender);

        // Wait for the thread to finish writing and exit.
        match self.handle.join() {
            Ok(()) => {}
            Err(_) => {
                panic!("writer thread panicked");
            }
        }
    }
}

#[derive(Debug)]
struct RecordReporterWriter {
    run_recorder: RunRecorder,
}

impl RecordReporterWriter {
    fn handle_event(&mut self, event: RecordEvent) -> Result<(), RecordReporterError> {
        match event {
            RecordEvent::Meta {
                cargo_metadata_json,
                test_list,
            } => self
                .run_recorder
                .write_meta(&cargo_metadata_json, &test_list)
                .map_err(RecordReporterError::RunStore),
            RecordEvent::TestEvent(event) => self
                .run_recorder
                .write_event(event)
                .map_err(RecordReporterError::RunStore),
        }
    }

    fn finish(self) -> Result<(), RecordReporterError> {
        self.run_recorder
            .finish()
            .map_err(RecordReporterError::RunStore)
    }
}

#[derive(Debug)]
enum RecordEvent {
    Meta {
        cargo_metadata_json: Arc<String>,
        test_list: TestListSummary,
    },
    TestEvent(TestEventSummary<InMemoryOutput>),
}
