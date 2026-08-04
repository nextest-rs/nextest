// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A next-generation test runner for Buck2.
//!
//! `buck2-nextest` sits at Buck2's orchestrator layer: it reads the external
//! runner spec Buck2 derives from a target's `ExternalRunnerTestInfo` provider,
//! turns it into nextest's binary list, and drives nextest's runner over it.
//!
//! Only Rust test targets are supported, since nextest lists and runs tests over
//! the libtest protocol.

pub mod cli;
pub mod convert;
pub mod errors;
pub mod executor;
pub mod proto;
pub mod run;
pub mod spec;

#[cfg(feature = "spec-file")]
pub mod spec_cli;
