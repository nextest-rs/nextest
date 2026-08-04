// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Buck2's test executor protocol.
//!
//! `buck2 test` launches an external test executor and speaks gRPC to it over a
//! pair of pre-connected sockets. Two services are involved, and each side
//! implements one of them:
//!
//! * [`TestExecutor`](generated::test_executor_server::TestExecutor), which
//!   `buck2-nextest` serves. Buck2 calls it to deliver one
//!   [`ExternalRunnerSpec`](generated::ExternalRunnerSpec) per configured test
//!   target, then to say there are no more.
//! * [`TestOrchestrator`](generated::test_orchestrator_client::TestOrchestratorClient),
//!   which Buck2 serves. `buck2-nextest` calls it to resolve what to run, to
//!   report results, and to end the session.
//!
//! Both directions of both services are generated. The unused halves let a test
//! stand in for Buck2 without needing a live daemon.
//!
//! See `proto/test.proto` for where the protocol definition comes from and what
//! was changed on the way in.

mod generated;

pub use generated::*;
