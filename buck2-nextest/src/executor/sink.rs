// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Streaming test results back to Buck2 as they happen.
//!
//! Nextest hands results to a callback that runs on one of its own runtime's
//! worker threads. That callback must not block -- while it is running, no test
//! start, completion, signal, or timeout is processed -- and it cannot block on
//! a future, since blocking inside a runtime panics. Sending a gRPC request
//! from it directly is therefore not an option.
//!
//! So the callback does the cheapest thing that preserves the information: it
//! converts the borrowed event into an owned message and pushes it onto a
//! bounded channel. A dedicated thread drains that channel and makes the calls.
//! This mirrors `nextest-runner`'s own `RecordReporter`, which solves the same
//! problem for on-disk recordings.
//!
//! The channel is bounded, so a Buck2 that stops reading applies backpressure
//! rather than letting results pile up without limit.

use crate::{
    errors::ExpectedError,
    proto::{
        ConfiguredTargetHandle, ReportTestResultRequest, TestResult, TestStatus,
        test_orchestrator_client::TestOrchestratorClient, test_result::OptionalMsg,
    },
};
use nextest_metadata::RustBinaryId;
use nextest_runner::{
    helpers::plural,
    reporter::events::{ExecutionResultDescription, ReporterEvent, TestEventKind},
};
use std::{
    collections::HashMap,
    sync::mpsc::{self, SyncSender, TrySendError},
    thread,
    time::Duration,
};
use tokio::runtime::Handle;
use tonic::transport::Channel;

/// How many results may be in flight before the callback starts waiting.
///
/// Large enough that a normal run never blocks on it, small enough that a
/// wedged Buck2 is noticed rather than absorbed.
const CHANNEL_DEPTH: usize = 128;

/// One finished test, owned so it can outlive the borrowed event it came from.
#[derive(Clone, Debug)]
struct Finished {
    handle: ConfiguredTargetHandle,
    name: String,
    status: TestStatus,
    duration: Option<Duration>,
    details: String,
    message: Option<String>,
}

/// Reports results to Buck2 from a thread of its own.
#[derive(Debug)]
pub(super) struct ResultSink {
    sender: SyncSender<Finished>,
    handles: HashMap<RustBinaryId, ConfiguredTargetHandle>,
    worker: thread::JoinHandle<Result<(), ExpectedError>>,
}

impl ResultSink {
    /// Starts the reporting thread.
    ///
    /// `runtime` drives the gRPC calls. It must handle to a runtime with worker
    /// threads of its own, since the reporting thread blocks on each call
    /// rather than driving the runtime itself.
    pub(super) fn new(
        client: TestOrchestratorClient<Channel>,
        runtime: Handle,
        handles: HashMap<RustBinaryId, ConfiguredTargetHandle>,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Finished>(CHANNEL_DEPTH);
        let worker = thread::Builder::new()
            .name("buck2-nextest-results".to_owned())
            .spawn(move || {
                let mut client = client;
                while let Ok(finished) = receiver.recv() {
                    let request = ReportTestResultRequest {
                        result: Some(TestResult {
                            name: finished.name,
                            status: finished.status as i32,
                            msg: finished.message.map(|msg| OptionalMsg { msg }),
                            target: Some(finished.handle),
                            duration: finished.duration.map(duration_to_proto),
                            details: finished.details,
                            max_memory_used_bytes: None,
                        }),
                    };
                    runtime
                        .block_on(client.report_test_result(request))
                        .map_err(|status| ExpectedError::ReportResultsError {
                            status: Box::new(status),
                        })?;
                }
                Ok(())
            })
            .expect("spawning a thread with a valid name succeeds");

        Self {
            sender,
            handles,
            worker,
        }
    }

    /// Forwards an event, if it is one Buck2 cares about.
    ///
    /// Returns an error only when the reporting thread has stopped, which
    /// nextest turns into a graceful cancellation of the run.
    pub(super) fn write_event(&self, event: &ReporterEvent<'_>) -> Result<(), SinkDisconnected> {
        let ReporterEvent::Test(event) = event else {
            return Ok(());
        };

        let finished = match &event.kind {
            TestEventKind::TestFinished {
                test_instance,
                run_statuses,
                ..
            } => {
                let last = run_statuses.last_status();
                let attempts = run_statuses.len();
                Finished {
                    handle: self.handle_for(test_instance.binary_id),
                    name: test_instance.test_name.to_string(),
                    status: status_for(&last.result),
                    duration: Some(last.time_taken),
                    details: last
                        .error_summary
                        .as_ref()
                        .map_or_else(String::new, |summary| summary.description.clone()),
                    // Buck2 only sees the final status, so a test that needed
                    // more than one attempt says so.
                    message: (attempts > 1).then(|| {
                        format!("{attempts} {} were made", plural::attempts_str(attempts))
                    }),
                }
            }
            TestEventKind::TestSkipped { test_instance, .. } => Finished {
                handle: self.handle_for(test_instance.binary_id),
                name: test_instance.test_name.to_string(),
                status: TestStatus::Skip,
                duration: None,
                details: String::new(),
                message: None,
            },
            _ => return Ok(()),
        };

        match self.sender.try_send(finished) {
            Ok(()) => Ok(()),
            // A full channel means Buck2 is slow, not gone: wait for it.
            Err(TrySendError::Full(finished)) => {
                self.sender.send(finished).map_err(|_| SinkDisconnected)
            }
            Err(TrySendError::Disconnected(_)) => Err(SinkDisconnected),
        }
    }

    /// Stops the reporting thread and waits for the backlog to drain.
    pub(super) fn finish(self) -> Result<(), ExpectedError> {
        let Self { sender, worker, .. } = self;
        // Dropping the sender is what ends the receive loop.
        drop(sender);
        worker
            .join()
            .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
    }

    fn handle_for(&self, binary_id: &RustBinaryId) -> ConfiguredTargetHandle {
        // Every binary in the list came from a spec, so a miss would mean the
        // two got out of step: a bug here, not a runtime condition.
        *self
            .handles
            .get(binary_id)
            .unwrap_or_else(|| panic!("no Buck2 target handle for binary `{binary_id}`"))
    }
}

/// The reporting thread is gone, so no further results can be delivered.
#[derive(Clone, Copy, Debug)]
pub(super) struct SinkDisconnected;

/// Maps nextest's outcome onto the protocol's.
///
/// Buck2's vocabulary is coarser than nextest's: it has no way to say "leaked a
/// handle" or "passed on the second attempt". Anything nextest counts as a
/// success is reported as a pass, so Buck2's summary agrees with nextest's.
fn status_for(result: &ExecutionResultDescription) -> TestStatus {
    if result.is_success() {
        return TestStatus::Pass;
    }
    match result {
        ExecutionResultDescription::Timeout { .. } => TestStatus::Timeout,
        // A test that could not be executed at all is an infrastructure
        // problem rather than a test that failed on its own terms.
        ExecutionResultDescription::ExecFail => TestStatus::Fatal,
        _ => TestStatus::Fail,
    }
}

fn duration_to_proto(duration: Duration) -> prost_types::Duration {
    // A wall-clock test duration cannot realistically overflow an i64 of
    // seconds; saturate rather than panic if one somehow does.
    prost_types::Duration {
        seconds: i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        nanos: i32::try_from(duration.subsec_nanos()).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nextest_runner::{
        config::elements::{LeakTimeoutResult, SlowTimeoutResult},
        reporter::events::FailureDescription,
    };

    #[test]
    fn nextest_outcomes_map_onto_buck2_statuses() {
        assert_eq!(
            status_for(&ExecutionResultDescription::Pass),
            TestStatus::Pass
        );
        assert_eq!(
            status_for(&ExecutionResultDescription::Leak {
                result: LeakTimeoutResult::Pass
            }),
            TestStatus::Pass,
            "a leak that nextest forgives is still a pass to Buck2"
        );
        assert_eq!(
            status_for(&ExecutionResultDescription::Leak {
                result: LeakTimeoutResult::Fail
            }),
            TestStatus::Fail,
        );
        assert_eq!(
            status_for(&ExecutionResultDescription::Timeout {
                result: SlowTimeoutResult::Fail
            }),
            TestStatus::Timeout,
        );
        assert_eq!(
            status_for(&ExecutionResultDescription::Timeout {
                result: SlowTimeoutResult::Pass
            }),
            TestStatus::Pass,
            "a timeout configured to pass is a pass, not a timeout failure"
        );
        assert_eq!(
            status_for(&ExecutionResultDescription::ExecFail),
            TestStatus::Fatal,
        );
        assert_eq!(
            status_for(&ExecutionResultDescription::Fail {
                failure: FailureDescription::ExitCode { code: 101 },
                leaked: false,
            }),
            TestStatus::Fail,
        );
    }

    #[test]
    fn durations_survive_conversion() {
        let converted = duration_to_proto(Duration::new(3, 500_000_000));
        assert_eq!(converted.seconds, 3);
        assert_eq!(converted.nanos, 500_000_000);
    }
}
