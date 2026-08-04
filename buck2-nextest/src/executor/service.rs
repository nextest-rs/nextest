// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The `TestExecutor` service, which Buck2 calls to deliver test targets.
//!
//! Buck2 sends one [`ExternalRunnerSpec`] per configured test target as its
//! analysis produces them, then calls `EndOfTestRequests` to say there are no
//! more. Specs are accumulated rather than acted on as they arrive: nextest
//! builds one test list and runs it, so the run cannot start until the set is
//! known. See the module documentation on [`crate::executor`] for why.

use crate::proto::{
    Empty, ExternalRunnerSpec, ExternalRunnerSpecRequest, UnstableHeapDumpRequest,
    UnstableHeapDumpResponse, test_executor_server::TestExecutor,
};
use std::sync::Mutex;
use tokio::sync::oneshot;
use tonic::{Request, Response, Status};

/// Collects the specs Buck2 sends, and signals when they stop.
#[derive(Debug)]
pub(super) struct SpecCollector {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    specs: Vec<ExternalRunnerSpec>,

    /// Dropped once `EndOfTestRequests` arrives, which wakes the receiver.
    ///
    /// `None` afterwards, so a second call is a no-op rather than a panic.
    done: Option<oneshot::Sender<Vec<ExternalRunnerSpec>>>,
}

impl SpecCollector {
    /// Creates a collector, along with the receiver for the completed set.
    pub(super) fn new() -> (Self, oneshot::Receiver<Vec<ExternalRunnerSpec>>) {
        let (done, rx) = oneshot::channel();
        let collector = Self {
            state: Mutex::new(State {
                specs: Vec::new(),
                done: Some(done),
            }),
        };
        (collector, rx)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A panic inside one of these short critical sections would mean a bug
        // in this file, not something to recover from.
        self.state
            .lock()
            .unwrap_or_else(|error| panic!("spec collector mutex was poisoned: {error}"))
    }
}

#[tonic::async_trait]
impl TestExecutor for SpecCollector {
    async fn external_runner_spec(
        &self,
        request: Request<ExternalRunnerSpecRequest>,
    ) -> Result<Response<Empty>, Status> {
        let spec = request.into_inner().test_spec.ok_or_else(|| {
            Status::invalid_argument("ExternalRunnerSpecRequest has no test_spec")
        })?;

        let mut state = self.lock();
        if state.done.is_none() {
            return Err(Status::failed_precondition(
                "a test spec arrived after EndOfTestRequests",
            ));
        }
        state.specs.push(spec);

        Ok(Response::new(Empty {}))
    }

    async fn end_of_test_requests(&self, _: Request<Empty>) -> Result<Response<Empty>, Status> {
        let mut state = self.lock();
        let Some(done) = state.done.take() else {
            return Err(Status::failed_precondition(
                "EndOfTestRequests was called more than once",
            ));
        };
        let specs = std::mem::take(&mut state.specs);
        drop(state);

        // The receiver is only gone if the run was already torn down, in which
        // case there is nothing left to tell.
        let _ = done.send(specs);

        Ok(Response::new(Empty {}))
    }

    async fn unstable_heap_dump(
        &self,
        _: Request<UnstableHeapDumpRequest>,
    ) -> Result<Response<UnstableHeapDumpResponse>, Status> {
        Err(Status::unimplemented(
            "buck2-nextest does not support heap dumps",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{ConfiguredTarget, ExternalRunnerSpecValue, external_runner_spec_value};

    fn spec(target: &str) -> ExternalRunnerSpecRequest {
        ExternalRunnerSpecRequest {
            test_spec: Some(ExternalRunnerSpec {
                target: Some(ConfiguredTarget {
                    cell: "root".to_owned(),
                    package: String::new(),
                    target: target.to_owned(),
                    ..ConfiguredTarget::default()
                }),
                test_type: "rust".to_owned(),
                command: vec![ExternalRunnerSpecValue {
                    value: Some(external_runner_spec_value::Value::Verbatim(
                        "/fake/binary".to_owned(),
                    )),
                }],
                ..ExternalRunnerSpec::default()
            }),
        }
    }

    #[tokio::test]
    async fn specs_are_delivered_in_arrival_order() {
        let (collector, rx) = SpecCollector::new();

        for target in ["a", "b", "c"] {
            collector
                .external_runner_spec(Request::new(spec(target)))
                .await
                .expect("spec is accepted");
        }
        collector
            .end_of_test_requests(Request::new(Empty {}))
            .await
            .expect("end of requests is accepted");

        let specs = rx.await.expect("specs are delivered");
        let targets: Vec<_> = specs
            .iter()
            .map(|spec| spec.target.as_ref().expect("target is set").target.as_str())
            .collect();
        assert_eq!(targets, ["a", "b", "c"]);
    }

    /// An empty run is legitimate -- Buck2 may match no test targets -- and must
    /// not hang waiting for specs that will never arrive.
    #[tokio::test]
    async fn no_specs_still_completes() {
        let (collector, rx) = SpecCollector::new();
        collector
            .end_of_test_requests(Request::new(Empty {}))
            .await
            .expect("end of requests is accepted");
        assert!(rx.await.expect("specs are delivered").is_empty());
    }

    #[tokio::test]
    async fn a_spec_without_a_target_is_rejected() {
        let (collector, _rx) = SpecCollector::new();
        let status = collector
            .external_runner_spec(Request::new(ExternalRunnerSpecRequest { test_spec: None }))
            .await
            .expect_err("a spec with no payload is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn a_second_end_of_requests_is_rejected() {
        let (collector, _rx) = SpecCollector::new();
        collector
            .end_of_test_requests(Request::new(Empty {}))
            .await
            .expect("the first call is accepted");
        let status = collector
            .end_of_test_requests(Request::new(Empty {}))
            .await
            .expect_err("the second call is rejected");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    }

    /// Accepting a spec after the set was closed would silently drop it, since
    /// the run has already been handed the specs it will use.
    #[tokio::test]
    async fn a_late_spec_is_rejected() {
        let (collector, _rx) = SpecCollector::new();
        collector
            .end_of_test_requests(Request::new(Empty {}))
            .await
            .expect("end of requests is accepted");
        let status = collector
            .external_runner_spec(Request::new(spec("late")))
            .await
            .expect_err("a late spec is rejected");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    }
}
