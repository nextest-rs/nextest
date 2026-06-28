// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{cmp::Ordering, fmt, sync::LazyLock};
use swrite::{SWrite, swrite};

/// OS scheduling priority for test processes. Maps to `nice` values on Unix and
/// to a [priority
/// class](https://learn.microsoft.com/en-us/windows/win32/procthread/scheduling-priorities)
/// on Windows.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CpuPriority {
    /// Do not alter the priority, instead inheriting nextest's own priority.
    #[default]
    Unset,

    /// Alter the priority, setting it to the provided level.
    Set(CpuPriorityLevel),
}

impl CpuPriority {
    // TODO-RAINCLAUDE: the level to apply, or None for Unset. This is the single place that discriminates the two variants, so a future variant fails to compile in exactly one spot rather than at scattered matches.
    pub fn level(self) -> Option<CpuPriorityLevel> {
        match self {
            CpuPriority::Unset => None,
            CpuPriority::Set(level) => Some(level),
        }
    }
}

/// A level to set the CPU priority to.
// TODO-RAINCLAUDE: Ord is implemented via rank() below (highest priority first) rather than derived, so `level < other` is a meaningful comparison: it means `level` is the higher priority. BTreeMap iteration over levels is therefore deterministic in priority order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "config-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum CpuPriorityLevel {
    /// High priority: `nice -10` on Unix, `HIGH_PRIORITY_CLASS` on Windows.
    High,

    /// Above normal priority: `nice -5` on Unix, `ABOVE_NORMAL_PRIORITY_CLASS`
    /// on Windows.
    AboveNormal,

    /// Normal priority: `nice 0` on Unix, `NORMAL_PRIORITY_CLASS` on Windows.
    Normal,

    /// Below normal priority: `nice +5` on Unix, `BELOW_NORMAL_PRIORITY_CLASS`
    /// on Windows.
    BelowNormal,

    /// Low priority: `nice +19` on Unix, `IDLE_PRIORITY_CLASS` on Windows.
    Low,
}

impl CpuPriorityLevel {
    // TODO-RAINCLAUDE: every level, sorted by rank() (highest priority first; asserted in tests). Feeds EXPECTED_VALUES, the probe-nice diagnostic, human report labeling, and the Windows warning order — update alongside rank() when adding a level.
    pub const ALL: [CpuPriorityLevel; 5] = [
        CpuPriorityLevel::High,
        CpuPriorityLevel::AboveNormal,
        CpuPriorityLevel::Normal,
        CpuPriorityLevel::BelowNormal,
        CpuPriorityLevel::Low,
    ];

    // TODO-RAINCLAUDE: canonical priority order, 0 = highest. Ord delegates to this, ALL is sorted by it, and the Windows denied/warned bitsets key off `1 << rank()`. On Unix it agrees with ascending to_nice() (asserted in tests).
    pub const fn rank(self) -> u8 {
        match self {
            CpuPriorityLevel::High => 0,
            CpuPriorityLevel::AboveNormal => 1,
            CpuPriorityLevel::Normal => 2,
            CpuPriorityLevel::BelowNormal => 3,
            CpuPriorityLevel::Low => 4,
        }
    }

    // TODO-RAINCLAUDE: kebab-case name of this level, matching the config syntax; used in user-facing messages. const so EXPECTED_VALUES below can be built from ALL at compile time.
    pub const fn as_str(self) -> &'static str {
        match self {
            CpuPriorityLevel::High => "high",
            CpuPriorityLevel::AboveNormal => "above-normal",
            CpuPriorityLevel::Normal => "normal",
            CpuPriorityLevel::BelowNormal => "below-normal",
            CpuPriorityLevel::Low => "low",
        }
    }

    // TODO-RAINCLAUDE: the Unix nice value for this level; conservative range [-10, 19] within [-NZERO, NZERO-1] (NZERO is 20 on most Unixes). Ref: https://pubs.opengroup.org/onlinepubs/9799919799/functions/getpriority.html
    #[cfg(unix)]
    pub fn to_nice(self) -> i32 {
        match self {
            CpuPriorityLevel::High => -10,
            CpuPriorityLevel::AboveNormal => -5,
            CpuPriorityLevel::Normal => 0,
            CpuPriorityLevel::BelowNormal => 5,
            CpuPriorityLevel::Low => 19,
        }
    }
}

impl PartialOrd for CpuPriorityLevel {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CpuPriorityLevel {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl Serialize for CpuPriority {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            CpuPriority::Unset => serializer.serialize_str("unset"),
            CpuPriority::Set(level) => level.serialize(serializer),
        }
    }
}

// TODO-RAINCLAUDE: the full set of config strings for CpuPriority ("unset" plus every level), built from ALL at compile time so a new level can't silently go missing from the unknown-variant error.
const EXPECTED_VALUES: [&str; CpuPriorityLevel::ALL.len() + 1] = {
    let mut values = ["unset"; CpuPriorityLevel::ALL.len() + 1];
    let mut i = 0;
    while i < CpuPriorityLevel::ALL.len() {
        values[i + 1] = CpuPriorityLevel::ALL[i].as_str();
        i += 1;
    }
    values
};

// TODO-RAINCLAUDE: the visitor's expecting message, derived from EXPECTED_VALUES so it can't go stale when a level is added (unlike a hand-written list).
static EXPECTING_MESSAGE: LazyLock<String> = LazyLock::new(|| {
    let mut msg = String::from("a CPU priority: ");
    let (last, rest) = EXPECTED_VALUES
        .split_last()
        .expect("EXPECTED_VALUES is non-empty");
    for value in rest {
        swrite!(msg, "\"{value}\", ");
    }
    swrite!(msg, "or \"{last}\"");
    msg
});

impl<'de> Deserialize<'de> for CpuPriority {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CpuPriorityVisitor;

        impl de::Visitor<'_> for CpuPriorityVisitor {
            type Value = CpuPriority;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&EXPECTING_MESSAGE)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value == "unset" {
                    return Ok(CpuPriority::Unset);
                }
                // TODO-RAINCLAUDE: delegate the level mapping to CpuPriorityLevel, but on an unknown value rebuild the error so "unset" appears alongside the levels (the level enum's own error omits it).
                let deserializer = de::value::StrDeserializer::new(value);
                CpuPriorityLevel::deserialize(deserializer)
                    .map(CpuPriority::Set)
                    .map_err(|_: E| E::unknown_variant(value, &EXPECTED_VALUES))
            }
        }

        deserializer.deserialize_str(CpuPriorityVisitor)
    }
}

#[cfg(feature = "config-schema")]
impl schemars::JsonSchema for CpuPriority {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CpuPriority".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // We do a tiny bit of trickery here -- we grab the CpuPriorityLevel
        // schema and splice in "unset" as a constant value. This avoids
        // unnecessary nesting of enum variants at the schema level.
        //
        // We do have to repeat the `unset` description here, though that isn't
        // too bad in the end.
        let unset = schemars::json_schema!({
            "description": "Do not alter the priority, instead inheriting \
                            nextest's own priority.",
            "type": "string",
            "const": "unset"
        });

        let mut level_schema = CpuPriorityLevel::json_schema(generator);
        let level_variants = level_schema
            .remove("oneOf")
            .and_then(|value| value.as_array().cloned())
            .expect("CpuPriorityLevel schema is a oneOf of documented string consts");

        let mut variants = vec![unset.to_value()];
        variants.extend(level_variants);

        schemars::json_schema!({
            "description": "OS scheduling priority for test processes. Maps to \
                            `nice` values on Unix and to a [priority \
                            class](https://learn.microsoft.com/en-us/windows/win32/procthread/scheduling-priorities) \
                            on Windows.",
            "oneOf": variants
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{core::NextestConfig, utils::test_helpers::*};
    use camino_tempfile::tempdir;
    use indoc::indoc;
    use nextest_filtering::ParseContext;
    use test_case::test_case;

    #[test]
    fn deserialize_values() {
        for (s, expected) in [
            ("\"unset\"", CpuPriority::Unset),
            ("\"high\"", CpuPriority::Set(CpuPriorityLevel::High)),
            (
                "\"above-normal\"",
                CpuPriority::Set(CpuPriorityLevel::AboveNormal),
            ),
            ("\"normal\"", CpuPriority::Set(CpuPriorityLevel::Normal)),
            (
                "\"below-normal\"",
                CpuPriority::Set(CpuPriorityLevel::BelowNormal),
            ),
            ("\"low\"", CpuPriority::Set(CpuPriorityLevel::Low)),
        ] {
            let actual: CpuPriority =
                serde_json::from_str(s).unwrap_or_else(|err| panic!("{s} should parse: {err}"));
            assert_eq!(actual, expected, "{s} deserializes correctly");
        }
    }

    #[test]
    fn deserialize_unknown_is_rejected() {
        let err = serde_json::from_str::<CpuPriority>("\"highest\"")
            .expect_err("unknown level should be rejected");
        assert!(
            err.to_string().contains("unset"),
            "the error lists unset alongside the levels: {err}"
        );

        let res: Result<CpuPriority, _> = serde_json::from_str("\"above_normal\"");
        res.expect_err("snake_case level should be rejected");
    }

    #[test]
    fn all_is_sorted_by_rank() {
        for (index, level) in CpuPriorityLevel::ALL.iter().enumerate() {
            assert_eq!(
                level.rank() as usize,
                index,
                "ALL is sorted by rank ({} is out of place)",
                level.as_str(),
            );
        }
    }

    #[test]
    fn ord_is_priority_order() {
        for pair in CpuPriorityLevel::ALL.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{} (higher priority) sorts before {}",
                pair[0].as_str(),
                pair[1].as_str(),
            );
        }
        #[cfg(unix)]
        for pair in CpuPriorityLevel::ALL.windows(2) {
            assert!(
                pair[0].to_nice() < pair[1].to_nice(),
                "rank order agrees with ascending nice ({} vs {})",
                pair[0].as_str(),
                pair[1].as_str(),
            );
        }
    }

    #[test]
    fn expecting_message_lists_every_value() {
        let err = serde_json::from_str::<CpuPriority>("5")
            .expect_err("a non-string cpu-priority should be rejected");
        let msg = err.to_string();
        for value in EXPECTED_VALUES {
            assert!(
                msg.contains(value),
                "the invalid-type error names {value}: {msg}"
            );
        }
    }

    #[cfg(feature = "config-schema")]
    #[test]
    fn schema_is_a_flat_documented_enum() {
        let schema = schemars::schema_for!(CpuPriority);

        assert!(
            schema
                .get("description")
                .and_then(|d| d.as_str())
                .is_some_and(|d| !d.is_empty()),
            "CpuPriority schema has a top-level description: {schema:?}",
        );

        let one_of = schema
            .get("oneOf")
            .and_then(|value| value.as_array())
            .expect("CpuPriority schema is a oneOf");

        let mut consts = Vec::new();
        for variant in one_of {
            assert_eq!(
                variant.get("type").and_then(|ty| ty.as_str()),
                Some("string"),
                "each variant is a string: {variant}",
            );
            let value = variant
                .get("const")
                .and_then(|c| c.as_str())
                .unwrap_or_else(|| panic!("each variant has a string const: {variant}"));
            let description = variant
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or_else(|| panic!("each variant is documented: {variant}"));
            assert!(
                !description.is_empty(),
                "{value} has a non-empty description",
            );
            consts.push(value.to_owned());
        }

        assert_eq!(
            consts,
            [
                "unset",
                "high",
                "above-normal",
                "normal",
                "below-normal",
                "low"
            ],
        );
    }

    #[test_case(
        "",
        CpuPriority::Unset,
        None
        ; "unset by default"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            cpu-priority = "unset"
        "#},
        CpuPriority::Unset,
        None
        ; "explicitly unset at the default profile"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            cpu-priority = "low"
        "#},
        CpuPriority::Set(CpuPriorityLevel::Low),
        None
        ; "set at the default profile"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            cpu-priority = "below-normal"

            [profile.ci]
            cpu-priority = "high"
        "#},
        CpuPriority::Set(CpuPriorityLevel::BelowNormal),
        Some(CpuPriority::Set(CpuPriorityLevel::High))
        ; "custom profile overrides the default"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            cpu-priority = "below-normal"

            [profile.ci]
            cpu-priority = "unset"
        "#},
        CpuPriority::Set(CpuPriorityLevel::BelowNormal),
        Some(CpuPriority::Unset)
        ; "custom profile unsets the default"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            cpu-priority = "below-normal"

            [profile.ci]
        "#},
        CpuPriority::Set(CpuPriorityLevel::BelowNormal),
        Some(CpuPriority::Set(CpuPriorityLevel::BelowNormal))
        ; "custom profile inherits the default"
    )]
    fn cpu_priority_adheres_to_hierarchy(
        config_contents: &str,
        expected_default: CpuPriority,
        maybe_expected_ci: Option<CpuPriority>,
    ) {
        let workspace_dir = tempdir().unwrap();
        let graph = temp_workspace(&workspace_dir, config_contents);
        let pcx = ParseContext::new(&graph);

        let nextest_config = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &Default::default(),
        )
        .expect("config file should parse");

        assert_eq!(
            nextest_config
                .profile("default")
                .expect("default profile should exist")
                .apply_build_platforms(&build_platforms())
                .cpu_priority(),
            expected_default,
        );

        if let Some(expected_ci) = maybe_expected_ci {
            assert_eq!(
                nextest_config
                    .profile("ci")
                    .expect("ci profile should exist")
                    .apply_build_platforms(&build_platforms())
                    .cpu_priority(),
                expected_ci,
            );
        }
    }
}
