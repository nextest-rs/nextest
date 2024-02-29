// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration support for nextest.

mod archive_include;
mod config_impl;
mod identifier;
mod nextest_version;
mod overrides;
mod retry_policy;
mod scripts;
mod slow_timeout;
mod test_group;
mod test_threads;
mod threads_required;
mod tool_config;

pub use archive_include::*;
pub use config_impl::*;
pub use identifier::*;
pub use nextest_version::*;
pub use overrides::*;
pub use retry_policy::*;
pub(super) use scripts::*;
pub use slow_timeout::*;
pub use test_group::*;
pub use test_threads::*;
pub use threads_required::*;
pub use tool_config::*;

#[cfg(test)]
mod test_helpers;
