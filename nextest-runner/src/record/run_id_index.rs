// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Index for run IDs enabling efficient prefix lookup and shortest unique prefix computation.

use super::store::RecordedRunInfo;
use crate::errors::InvalidRunIdSelector;
use quick_junit::ReportUuid;
use std::{fmt, str::FromStr};

/// Selector for identifying a run, either the most recent or by prefix.
///
/// This is used by CLI commands that need to specify a run ID. The `Latest`
/// variant selects the most recent completed run, while `Prefix` allows
/// specifying a run by its ID prefix.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RunIdSelector {
    /// Select the most recent completed run.
    #[default]
    Latest,

    /// Select a run by ID prefix.
    ///
    /// The prefix contains only hex digits and optional dashes (for UUID format).
    Prefix(String),
}

impl FromStr for RunIdSelector {
    type Err = InvalidRunIdSelector;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "latest" {
            Ok(RunIdSelector::Latest)
        } else {
            // Validate that the prefix contains only hex digits and dashes.
            let is_valid = !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
            if is_valid {
                Ok(RunIdSelector::Prefix(s.to_owned()))
            } else {
                Err(InvalidRunIdSelector {
                    input: s.to_owned(),
                })
            }
        }
    }
}

impl fmt::Display for RunIdSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunIdSelector::Latest => write!(f, "latest"),
            RunIdSelector::Prefix(prefix) => write!(f, "{prefix}"),
        }
    }
}

/// An index of run IDs enabling efficient prefix lookup and shortest unique
/// prefix computation.
///
/// This uses a sorted index with neighbor comparison (inspired by jujutsu's
/// approach) rather than a trie. For each ID, the shortest unique prefix is
/// determined by comparing with the lexicographically adjacent IDs—the minimum
/// prefix length needed to distinguish from both neighbors.
#[derive(Clone, Debug)]
pub struct RunIdIndex {
    /// Run IDs paired with their normalized hex representation, sorted by hex.
    sorted_entries: Vec<RunIdIndexEntry>,
}

/// An entry in the run ID index.
#[derive(Clone, Debug)]
struct RunIdIndexEntry {
    run_id: ReportUuid,
    /// Normalized hex representation (lowercase, no dashes).
    hex: String,
}

impl RunIdIndex {
    /// Creates a new index from a list of runs.
    pub fn new(runs: &[RecordedRunInfo]) -> Self {
        let mut sorted_entries: Vec<_> = runs
            .iter()
            .map(|r| RunIdIndexEntry {
                run_id: r.run_id,
                hex: r.run_id.to_string().replace('-', "").to_lowercase(),
            })
            .collect();

        // Sort by normalized hex representation for consistent ordering.
        sorted_entries.sort_by(|a, b| a.hex.cmp(&b.hex));
        Self { sorted_entries }
    }

    /// Returns the shortest unique prefix length for the given run ID.
    ///
    /// The returned length is in hex characters (not including dashes). Returns `None` if the
    /// run ID is not in the index.
    pub fn shortest_unique_prefix_len(&self, run_id: ReportUuid) -> Option<usize> {
        // Find the position of this ID in the sorted list.
        let pos = self
            .sorted_entries
            .iter()
            .position(|entry| entry.run_id == run_id)?;

        let target_hex = &self.sorted_entries[pos].hex;

        // Compare with neighbors to find the minimum distinguishing prefix length.
        let mut min_len = 1; // At least 1 character.

        // Compare with previous neighbor.
        if pos > 0 {
            let prev_hex = &self.sorted_entries[pos - 1].hex;
            let common = common_hex_prefix_len(target_hex, prev_hex);
            min_len = min_len.max(common + 1);
        }

        // Compare with next neighbor.
        if pos + 1 < self.sorted_entries.len() {
            let next_hex = &self.sorted_entries[pos + 1].hex;
            let common = common_hex_prefix_len(target_hex, next_hex);
            min_len = min_len.max(common + 1);
        }

        Some(min_len)
    }

    /// Returns the shortest unique prefix for the given run ID.
    ///
    /// The prefix is the minimum string needed to uniquely identify this run
    /// among all runs in the index. Both parts include dashes in the standard
    /// UUID positions.
    ///
    /// Returns `None` if the run ID is not in the index.
    pub fn shortest_unique_prefix(&self, run_id: ReportUuid) -> Option<ShortestRunIdPrefix> {
        let prefix_len = self.shortest_unique_prefix_len(run_id)?;
        Some(ShortestRunIdPrefix::new(run_id, prefix_len))
    }

    /// Resolves a prefix to a run ID.
    ///
    /// The prefix can include or omit dashes. Returns `Ok(run_id)` if exactly
    /// one run matches, or an error if none or multiple match.
    pub fn resolve_prefix(&self, prefix: &str) -> Result<ReportUuid, PrefixResolutionError> {
        // Validate and normalize the prefix.
        let normalized = prefix.replace('-', "").to_lowercase();
        if !normalized.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(PrefixResolutionError::InvalidPrefix);
        }

        // Find all matching IDs using binary search for the range.
        // First, find the start of the range.
        let start = self
            .sorted_entries
            .partition_point(|entry| entry.hex.as_str() < normalized.as_str());

        // Collect matches: all entries whose hex starts with the normalized prefix.
        let matches: Vec<_> = self.sorted_entries[start..]
            .iter()
            .take_while(|entry| entry.hex.starts_with(&normalized))
            .map(|entry| entry.run_id)
            .collect();

        match matches.len() {
            0 => Err(PrefixResolutionError::NotFound),
            1 => Ok(matches[0]),
            n => {
                let candidates = matches.into_iter().take(8).collect();
                Err(PrefixResolutionError::Ambiguous {
                    count: n,
                    candidates,
                })
            }
        }
    }

    /// Returns the number of run IDs in the index.
    pub fn len(&self) -> usize {
        self.sorted_entries.len()
    }

    /// Returns true if the index is empty.
    pub fn is_empty(&self) -> bool {
        self.sorted_entries.is_empty()
    }

    /// Returns an iterator over all run IDs in sorted order.
    pub fn iter(&self) -> impl Iterator<Item = ReportUuid> + '_ {
        self.sorted_entries.iter().map(|entry| entry.run_id)
    }
}

/// The shortest unique prefix for a run ID, split into the unique prefix and remaining portion.
///
/// This is useful for display purposes where the unique prefix can be highlighted differently
/// (e.g., in a different color) from the rest of the ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortestRunIdPrefix {
    /// The unique prefix portion (the minimum needed to identify this run).
    pub prefix: String,
    /// The remaining portion of the run ID.
    pub rest: String,
}

impl ShortestRunIdPrefix {
    /// Creates a new shortest prefix by splitting a UUID at the given hex character count.
    ///
    /// The `hex_len` is the number of hex characters (not including dashes) for the prefix.
    fn new(run_id: ReportUuid, hex_len: usize) -> Self {
        let full = run_id.to_string();

        // The UUID format is xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx.
        // The dash positions (0-indexed) are at 8, 13, 18, 23.
        // We need to find the string index that corresponds to `hex_len` hex characters.
        let split_index = hex_len_to_string_index(hex_len);
        let split_index = split_index.min(full.len());

        let (prefix, rest) = full.split_at(split_index);
        Self {
            prefix: prefix.to_string(),
            rest: rest.to_string(),
        }
    }

    /// Returns the full run ID by concatenating prefix and rest.
    pub fn full(&self) -> String {
        format!("{}{}", self.prefix, self.rest)
    }
}

/// Converts a hex character count to a string index in UUID format.
///
/// UUID format: `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`
/// - Positions 0-7: 8 hex chars
/// - Position 8: dash
/// - Positions 9-12: 4 hex chars (total 12)
/// - Position 13: dash
/// - Positions 14-17: 4 hex chars (total 16)
/// - Position 18: dash
/// - Positions 19-22: 4 hex chars (total 20)
/// - Position 23: dash
/// - Positions 24-35: 12 hex chars (total 32)
fn hex_len_to_string_index(hex_len: usize) -> usize {
    // Count how many dashes come before the hex_len'th hex character.
    let dashes = match hex_len {
        0..=8 => 0,
        9..=12 => 1,
        13..=16 => 2,
        17..=20 => 3,
        21..=32 => 4,
        _ => 4, // Max 32 hex chars in a UUID.
    };
    hex_len + dashes
}

/// Computes the length of the common prefix between two hex strings.
fn common_hex_prefix_len(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(ca, cb)| ca == cb)
        .count()
}

/// Internal error type for prefix resolution.
///
/// This is converted to [`crate::errors::RunIdResolutionError`] by
/// [`super::store::RunStoreSnapshot::resolve_run_id`], which can enrich the
/// error with full `RecordedRunInfo` data.
#[derive(Clone, Debug)]
pub enum PrefixResolutionError {
    /// No run found matching the prefix.
    NotFound,

    /// Multiple runs match the prefix.
    Ambiguous {
        /// The total number of matching runs.
        count: usize,
        /// The candidates that matched (up to a limit).
        candidates: Vec<ReportUuid>,
    },

    /// The prefix contains invalid characters (expected hexadecimal).
    InvalidPrefix,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{RecordedRunStatus, RecordedSizes, format::RECORD_FORMAT_VERSION};
    use chrono::TimeZone;
    use semver::Version;
    use std::collections::BTreeMap;

    /// Creates a test run with the given run ID.
    fn make_run(run_id: ReportUuid) -> RecordedRunInfo {
        let started_at = chrono::FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
            .unwrap();
        RecordedRunInfo {
            run_id,
            store_format_version: RECORD_FORMAT_VERSION,
            nextest_version: Version::new(0, 1, 0),
            started_at,
            last_written_at: started_at,
            duration_secs: None,
            cli_args: Vec::new(),
            build_scope_args: Vec::new(),
            env_vars: BTreeMap::new(),
            parent_run_id: None,
            sizes: RecordedSizes::default(),
            status: RecordedRunStatus::Incomplete,
        }
    }

    #[test]
    fn test_empty_index() {
        let index = RunIdIndex::new(&[]);
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn test_single_entry() {
        let runs = vec![make_run(ReportUuid::from_u128(
            0x550e8400_e29b_41d4_a716_446655440000,
        ))];
        let index = RunIdIndex::new(&runs);

        assert_eq!(index.len(), 1);

        // With only one entry, shortest prefix is 1 character.
        assert_eq!(index.shortest_unique_prefix_len(runs[0].run_id), Some(1));

        let prefix = index.shortest_unique_prefix(runs[0].run_id).unwrap();
        assert_eq!(prefix.prefix, "5");
        assert_eq!(prefix.rest, "50e8400-e29b-41d4-a716-446655440000");
        assert_eq!(prefix.full(), "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn test_shared_prefix() {
        // Two UUIDs that share the first 4 hex characters "5555".
        let runs = vec![
            make_run(ReportUuid::from_u128(
                0x55551111_0000_0000_0000_000000000000,
            )),
            make_run(ReportUuid::from_u128(
                0x55552222_0000_0000_0000_000000000000,
            )),
        ];
        let index = RunIdIndex::new(&runs);

        // Both need 5 characters to be unique (shared "5555", differ at position 5).
        assert_eq!(index.shortest_unique_prefix_len(runs[0].run_id), Some(5));
        assert_eq!(index.shortest_unique_prefix_len(runs[1].run_id), Some(5));

        let prefix0 = index.shortest_unique_prefix(runs[0].run_id).unwrap();
        assert_eq!(prefix0.prefix, "55551");
        assert_eq!(prefix0.rest, "111-0000-0000-0000-000000000000");

        let prefix1 = index.shortest_unique_prefix(runs[1].run_id).unwrap();
        assert_eq!(prefix1.prefix, "55552");
    }

    #[test]
    fn test_asymmetric_neighbors() {
        // Three UUIDs where prefix lengths differ based on neighbors.
        // 1111... < 1112... < 2222...
        let runs = vec![
            make_run(ReportUuid::from_u128(
                0x11110000_0000_0000_0000_000000000000,
            )),
            make_run(ReportUuid::from_u128(
                0x11120000_0000_0000_0000_000000000000,
            )),
            make_run(ReportUuid::from_u128(
                0x22220000_0000_0000_0000_000000000000,
            )),
        ];
        let index = RunIdIndex::new(&runs);

        // First two share "111", need 4 chars each.
        assert_eq!(index.shortest_unique_prefix_len(runs[0].run_id), Some(4));
        assert_eq!(index.shortest_unique_prefix_len(runs[1].run_id), Some(4));
        // Third differs at first char from its only neighbor.
        assert_eq!(index.shortest_unique_prefix_len(runs[2].run_id), Some(1));
    }

    #[test]
    fn test_prefix_crosses_dash() {
        // Prefix extends past the first dash (position 8). Share first 9 hex chars.
        let runs = vec![
            make_run(ReportUuid::from_u128(
                0x12345678_9000_0000_0000_000000000000,
            )),
            make_run(ReportUuid::from_u128(
                0x12345678_9111_0000_0000_000000000000,
            )),
        ];
        let index = RunIdIndex::new(&runs);

        assert_eq!(index.shortest_unique_prefix_len(runs[0].run_id), Some(10));

        // Prefix string includes the dash.
        let prefix = index.shortest_unique_prefix(runs[0].run_id).unwrap();
        assert_eq!(prefix.prefix, "12345678-90");
        assert_eq!(prefix.rest, "00-0000-0000-000000000000");
    }

    #[test]
    fn test_resolve_prefix() {
        let runs = vec![
            make_run(ReportUuid::from_u128(
                0xabcdef00_1234_5678_9abc_def012345678,
            )),
            make_run(ReportUuid::from_u128(
                0x22222222_2222_2222_2222_222222222222,
            )),
            make_run(ReportUuid::from_u128(
                0x23333333_3333_3333_3333_333333333333,
            )),
        ];
        let index = RunIdIndex::new(&runs);

        // Single match.
        assert_eq!(index.resolve_prefix("abc").unwrap(), runs[0].run_id);
        assert_eq!(index.resolve_prefix("22").unwrap(), runs[1].run_id);

        // Case insensitive.
        assert_eq!(index.resolve_prefix("ABC").unwrap(), runs[0].run_id);
        assert_eq!(index.resolve_prefix("AbC").unwrap(), runs[0].run_id);

        // With dashes.
        assert_eq!(index.resolve_prefix("abcdef00-").unwrap(), runs[0].run_id);
        assert_eq!(index.resolve_prefix("abcdef00-12").unwrap(), runs[0].run_id);

        // Ambiguous.
        let err = index.resolve_prefix("2").unwrap_err();
        assert!(matches!(
            err,
            PrefixResolutionError::Ambiguous { count: 2, .. }
        ));

        // Not found.
        let err = index.resolve_prefix("9").unwrap_err();
        assert!(matches!(err, PrefixResolutionError::NotFound));

        // Invalid.
        let err = index.resolve_prefix("xyz").unwrap_err();
        assert!(matches!(err, PrefixResolutionError::InvalidPrefix));
    }

    #[test]
    fn test_not_in_index() {
        let runs = vec![make_run(ReportUuid::from_u128(
            0x11111111_1111_1111_1111_111111111111,
        ))];
        let index = RunIdIndex::new(&runs);

        let other = ReportUuid::from_u128(0x22222222_2222_2222_2222_222222222222);
        assert_eq!(index.shortest_unique_prefix_len(other), None);
        assert_eq!(index.shortest_unique_prefix(other), None);
    }

    #[test]
    fn test_hex_len_to_string_index() {
        // Before first dash (position 8).
        assert_eq!(hex_len_to_string_index(0), 0);
        assert_eq!(hex_len_to_string_index(8), 8);
        // After each dash.
        assert_eq!(hex_len_to_string_index(9), 10);
        assert_eq!(hex_len_to_string_index(13), 15);
        assert_eq!(hex_len_to_string_index(17), 20);
        assert_eq!(hex_len_to_string_index(21), 25);
        // Full UUID.
        assert_eq!(hex_len_to_string_index(32), 36);
    }

    #[test]
    fn test_run_id_selector_default() {
        assert_eq!(RunIdSelector::default(), RunIdSelector::Latest);
    }

    #[test]
    fn test_run_id_selector_from_str() {
        // Only exact "latest" parses to Latest.
        assert_eq!(
            "latest".parse::<RunIdSelector>().unwrap(),
            RunIdSelector::Latest
        );

        // Valid hex prefixes.
        assert_eq!(
            "abc123".parse::<RunIdSelector>().unwrap(),
            RunIdSelector::Prefix("abc123".to_owned())
        );
        assert_eq!(
            "550e8400-e29b-41d4".parse::<RunIdSelector>().unwrap(),
            RunIdSelector::Prefix("550e8400-e29b-41d4".to_owned())
        );
        assert_eq!(
            "ABCDEF".parse::<RunIdSelector>().unwrap(),
            RunIdSelector::Prefix("ABCDEF".to_owned())
        );
        assert_eq!(
            "0".parse::<RunIdSelector>().unwrap(),
            RunIdSelector::Prefix("0".to_owned())
        );

        // "Latest" contains non-hex characters.
        assert!("Latest".parse::<RunIdSelector>().is_err());
        assert!("LATEST".parse::<RunIdSelector>().is_err());

        // Contains non-hex characters.
        assert!("xyz".parse::<RunIdSelector>().is_err());
        assert!("abc_123".parse::<RunIdSelector>().is_err());
        assert!("hello".parse::<RunIdSelector>().is_err());

        // Empty string is invalid.
        assert!("".parse::<RunIdSelector>().is_err());
    }

    #[test]
    fn test_run_id_selector_display() {
        assert_eq!(RunIdSelector::Latest.to_string(), "latest");
        assert_eq!(
            RunIdSelector::Prefix("abc123".to_owned()).to_string(),
            "abc123"
        );
    }
}
