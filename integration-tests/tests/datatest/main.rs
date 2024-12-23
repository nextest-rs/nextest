// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data-driven tests.

mod custom_target;
mod helpers;

datatest_stable::harness! {
    {
        test = custom_target::custom_invalid,
        root = &target_spec_miette::fixtures::CUSTOM_INVALID,
        pattern = r"^.*$",
    },
}
