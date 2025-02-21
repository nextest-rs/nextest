// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Experimental support for creating a Tokio console.
//!
//! This is currently not shipped with release builds but is available as a debugging tool.

use tracing::Subscriber;
use tracing_subscriber::{Layer, registry::LookupSpan};

/// Spawns the Tokio console subscriber in a background thread, returning a tracing `Layer` that
/// refers to it.
pub fn spawn<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    console_subscriber::spawn()
}
