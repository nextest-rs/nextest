// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The test runner.
//!
//! For more information about the design of the runner loop, see the design
//! document: [_The runner loop_].
//!
//! The main structure in this module is [`TestRunner`].
//!
//! [_The runner loop_]: https://nexte.st/docs/design/architecture/runner-loop/

mod dispatcher;
mod executor;
mod imp;
mod internal_events;
mod script_helpers;

#[cfg(unix)]
#[path = "unix.rs"]
mod os;

#[cfg(windows)]
#[path = "windows.rs"]
mod os;

use dispatcher::*;
use executor::*;
pub use imp::*;
use internal_events::*;
use script_helpers::*;
