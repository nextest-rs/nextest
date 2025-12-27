// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration elements for nextest.

mod archive;
pub(super) mod bench;
mod global_timeout;
mod inherits;
mod junit;
mod leak_timeout;
mod max_fail;
mod priority;
mod retry_policy;
pub(super) mod slow_timeout;
mod test_group;
mod test_threads;
mod threads_required;

pub use archive::*;
pub(super) use bench::*;
pub use global_timeout::*;
pub use inherits::*;
pub use junit::*;
pub use leak_timeout::*;
pub use max_fail::*;
pub use priority::*;
pub use retry_policy::*;
pub use slow_timeout::*;
pub use test_group::*;
pub use test_threads::*;
pub use threads_required::*;
