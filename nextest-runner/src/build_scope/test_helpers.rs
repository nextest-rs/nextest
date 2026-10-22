// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::PackageSpec;
use proptest::{option, prelude::*};

pub(crate) fn arb_valid_package_spec() -> impl Strategy<Value = PackageSpec> {
    let version =
        (0u64..100, option::of((0u64..100, option::of(0u64..100)))).prop_map(|(major, rest)| {
            match rest {
                None => format!("{major}"),
                Some((minor, None)) => format!("{major}.{minor}"),
                Some((minor, Some(patch))) => format!("{major}.{minor}.{patch}"),
            }
        });
    let name_spec =
        ("[a-z_][a-z0-9_-]{0,12}", option::of(version)).prop_map(|(name, version)| match version {
            None => name,
            Some(version) => format!("{name}@{version}"),
        });
    let glob_spec = prop_oneof![
        Just("nextest-*".to_owned()),
        Just("*-metadata".to_owned()),
        Just("fixture-?".to_owned()),
    ];

    prop_oneof![4 => name_spec, 1 => glob_spec]
        .prop_map(|spec| PackageSpec::new(&spec).expect("generated spec is valid"))
}
