// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Facilities for tracking Cargo build scopes.

mod package_spec;
mod scope;
#[cfg(test)]
pub(crate) mod test_helpers;

pub use package_spec::PackageSpec;
pub use scope::{
    BuildScope, CargoFlag, FeatureSelection, NamedTargets, PackageSelection, TargetSelection,
};
