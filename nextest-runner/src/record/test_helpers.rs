// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test helpers for proptest support in record types.

use super::summary::{OutputFileName, ZipStoreOutput};
use proptest::prelude::*;

/// Strategy for generating arbitrary [`OutputFileName`] values.
pub fn arb_output_file_name() -> impl Strategy<Value = OutputFileName> {
    // Generate valid output file names: 16 hex chars + suffix.
    let suffixes = prop_oneof![Just("-stdout"), Just("-stderr"), Just("-combined")];

    ("[0-9a-f]{16}", suffixes).prop_map(|(hash, suffix)| {
        // Use serde roundtrip to construct since the inner field is private.
        let json = format!(r#""{hash}{suffix}""#);
        serde_json::from_str(&json).expect("valid output file name")
    })
}

impl Arbitrary for OutputFileName {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        arb_output_file_name().boxed()
    }
}

impl Arbitrary for ZipStoreOutput {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        prop_oneof![
            Just(ZipStoreOutput::Empty),
            any::<OutputFileName>().prop_map(|file_name| ZipStoreOutput::Full { file_name }),
            (any::<OutputFileName>(), any::<u64>()).prop_map(|(file_name, original_size)| {
                ZipStoreOutput::Truncated {
                    file_name,
                    original_size,
                }
            }),
        ]
        .boxed()
    }
}
