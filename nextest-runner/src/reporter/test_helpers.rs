// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test helpers for proptest support in reporter types.

use crate::config::{core::ConfigIdentifier, scripts::ScriptId};
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use proptest::prelude::*;
use smol_str::SmolStr;

/// Strategy for generating arbitrary `Duration` values.
pub fn arb_duration() -> impl Strategy<Value = std::time::Duration> {
    any::<u64>().prop_map(std::time::Duration::from_millis)
}

/// Strategy for generating arbitrary `DateTime<FixedOffset>` values.
pub fn arb_datetime_fixed_offset() -> impl Strategy<Value = DateTime<FixedOffset>> {
    // Generate a timestamp in a reasonable range (year 2000-2100).
    // Offset must be in minutes (not seconds) to survive JSON round-tripping,
    // since RFC 3339 serialization truncates sub-minute offset precision.
    (946684800i64..4102444800i64, -720i32..720i32).prop_map(|(secs, offset_minutes)| {
        let offset_secs = offset_minutes * 60;
        let offset =
            FixedOffset::east_opt(offset_secs).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
        let utc = Utc
            .timestamp_opt(secs, 0)
            .single()
            .expect("valid timestamp");
        utc.with_timezone(&offset)
    })
}

/// Strategy for generating arbitrary `SmolStr` values.
pub fn arb_smol_str() -> impl Strategy<Value = SmolStr> {
    any::<String>().prop_map(SmolStr::new)
}

impl Arbitrary for ConfigIdentifier {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        // Generate valid identifiers: start with XID_Start, followed by XID_Continue or hyphens.
        // For simplicity, use ASCII letters and hyphens.
        let regular_identifier = "[a-zA-Z][a-zA-Z0-9-]{0,20}";
        let tool_identifier = "@tool:[a-zA-Z][a-zA-Z0-9-]{0,10}:[a-zA-Z][a-zA-Z0-9-]{0,10}";

        prop_oneof![
            3 => regular_identifier
                .prop_map(|s| ConfigIdentifier::new(s.into()).expect("valid identifier")),
            1 => tool_identifier
                .prop_map(|s| ConfigIdentifier::new(s.into()).expect("valid tool identifier")),
        ]
        .boxed()
    }
}

impl Arbitrary for ScriptId {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        any::<ConfigIdentifier>().prop_map(ScriptId).boxed()
    }
}
