// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration support for nextest.
//!
//! See the [nextest configuration reference](https://nexte.st/docs/configuration/reference/).
//!
//! ## Multi-pass parsing
//!
//! Nextest's configuration parsing happens in several passes, similar to a
//! typical compiler.
//!
//! * The first pass verifies that the configuration looks fine and is done
//!   very early in the process. A successful first phase parse is represented by
//!   [`EarlyProfile`](core::EarlyProfile).
//! * The second pass applies the host and target platforms to the configuration,
//!   resulting in an [`EvaluatableProfile`](core::EvaluatableProfile).
//! * The final pass resolves actual per-test settings, via [`TestSettings`](overrides::TestSettings).
//!
//! Multi-pass parsing allows for profile parsing errors to be returned as early
//! as possible -- before the host and target platforms are known. Returning
//! errors early leads to a better user experience.

pub mod core;
pub mod elements;
pub mod overrides;
pub mod scripts;
mod utils;
