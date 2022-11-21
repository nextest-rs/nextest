// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Experimental support for creating a Tokio console.
//!
//! This is currently not shipped with release builds but is available as a debugging tool.

/// Initializes the Tokio console subscriber.
pub fn init() {
    console_subscriber::init();
}
