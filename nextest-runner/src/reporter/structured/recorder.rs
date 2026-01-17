// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reporter for recording test runs to disk.

use crate::{
    errors::RecordReporterError,
    record::{RecordOpts, RunRecorder, StoreSizes, TestEventSummary},
    reporter::events::TestEvent,
    test_output::ChildSingleOutput,
};
use nextest_metadata::TestListSummary;
use std::{
    any::Any,
    sync::{Arc, mpsc},
    thread::JoinHandle,
};

/// A reporter that records test runs to disk.
///
/// This reporter runs in a separate thread, receiving events via a bounded
/// channel. Events are converted to serializable form and written to the
/// archive asynchronously.
#[derive(Debug)]
pub struct RecordReporter<'a> {
    // Invariant: sender is always Some while the reporter is alive.
    sender: Option<mpsc::SyncSender<RecordEvent>>,
    handle: JoinHandle<Result<StoreSizes, RecordReporterError>>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> RecordReporter<'a> {
    /// Creates a new `RecordReporter` with the given recorder.
    pub fn new(run_recorder: RunRecorder) -> Self {
        // Spawn a thread to do the writing. Use a bounded channel with backpressure.
        let (sender, receiver) = mpsc::sync_channel(128);
        let handle = std::thread::spawn(move || {
            let mut writer = RecordReporterWriter { run_recorder };
            while let Ok(event) = receiver.recv() {
                writer.handle_event(event)?;
            }

            // The sender has been dropped. Finish writing and exit.
            writer.finish()
        });

        Self {
            sender: Some(sender),
            handle,
            _marker: std::marker::PhantomData,
        }
    }

    /// Writes metadata to the recorder.
    ///
    /// This should be called once at the beginning of a test run.
    pub fn write_meta(
        &self,
        cargo_metadata_json: Arc<String>,
        test_list: TestListSummary,
        opts: RecordOpts,
    ) {
        let event = RecordEvent::Meta {
            cargo_metadata_json,
            test_list,
            opts,
        };
        // Ignore send errors because they indicate that the receiver has exited
        // (likely due to an error, which is dealt with in finish()).
        _ = self
            .sender
            .as_ref()
            .expect("sender is always Some")
            .send(event);
    }

    /// Writes a test event to the recorder.
    ///
    /// Events that should not be recorded (informational/interactive) are
    /// silently skipped.
    pub fn write_event(&self, event: TestEvent<'_>) {
        let Some(summary) = TestEventSummary::from_test_event(event) else {
            // Non-recordable event, skip it.
            return;
        };
        let event = RecordEvent::TestEvent(summary);
        // Ignore send errors because they indicate that the receiver has exited
        // (likely due to an error, which is dealt with in finish()).
        _ = self
            .sender
            .as_ref()
            .expect("sender is always Some")
            .send(event);
    }

    /// Finishes writing and waits for the recorder thread to exit.
    ///
    /// Returns the sizes of the recording (compressed and uncompressed), or an error if recording
    /// failed.
    ///
    /// This must be called before the reporter is dropped.
    pub fn finish(mut self) -> Result<StoreSizes, RecordReporterError> {
        // Drop the sender, which signals the receiver to exit.
        let sender = self.sender.take();
        std::mem::drop(sender);

        // Wait for the thread to finish writing and exit.
        match self.handle.join() {
            Ok(result) => result,
            Err(panic_payload) => Err(RecordReporterError::WriterPanic {
                message: panic_payload_to_string(panic_payload),
            }),
        }
    }
}

/// Extracts a string message from a panic payload.
fn panic_payload_to_string(payload: Box<dyn Any + Send + 'static>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(unknown panic payload)".to_owned()
    }
}

/// Internal writer that runs in the recording thread.
struct RecordReporterWriter {
    run_recorder: RunRecorder,
}

impl RecordReporterWriter {
    fn handle_event(&mut self, event: RecordEvent) -> Result<(), RecordReporterError> {
        match event {
            RecordEvent::Meta {
                cargo_metadata_json,
                test_list,
                opts,
            } => self
                .run_recorder
                .write_meta(&cargo_metadata_json, &test_list, &opts)
                .map_err(RecordReporterError::RunStore),
            RecordEvent::TestEvent(event) => self
                .run_recorder
                .write_event(event)
                .map_err(RecordReporterError::RunStore),
        }
    }

    fn finish(self) -> Result<StoreSizes, RecordReporterError> {
        self.run_recorder
            .finish()
            .map_err(RecordReporterError::RunStore)
    }
}

/// Events sent to the recording thread.
#[derive(Debug)]
enum RecordEvent {
    /// Metadata about the test run.
    Meta {
        cargo_metadata_json: Arc<String>,
        test_list: TestListSummary,
        opts: RecordOpts,
    },
    /// A test event.
    TestEvent(TestEventSummary<ChildSingleOutput>),
}
