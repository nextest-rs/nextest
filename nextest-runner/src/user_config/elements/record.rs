// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Record-related user configuration.

use crate::user_config::helpers::resolve_record_setting;
use bytesize::ByteSize;
use serde::Deserialize;
use std::time::Duration;
use target_spec::{Platform, TargetSpec};

/// Minimum allowed value for `max_output_size`.
///
/// This ensures there's enough space for the truncation marker plus some
/// actual content. The truncation marker is approximately 40 bytes, so 1000
/// bytes provides reasonable headroom.
pub const MIN_MAX_OUTPUT_SIZE: ByteSize = ByteSize::b(1000);

/// Maximum allowed value for `max_output_size`.
///
/// This caps how much output can be stored per test, bounding memory usage
/// when reading archives. Values above this are clamped with a warning.
///
/// This constant is also used as the maximum size for reading any file from
/// a recorded archive, preventing malicious archives from causing OOM.
pub const MAX_MAX_OUTPUT_SIZE: ByteSize = ByteSize::mib(256);

/// Record configuration in user config.
///
/// This section controls retention policies for recorded test runs.
/// All fields are optional; unspecified fields will use defaults.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DeserializedRecordConfig {
    /// Whether recording is enabled.
    ///
    /// This allows users to have recording configured but temporarily disabled.
    /// When false, no recording occurs even if the `record` experimental feature
    /// is enabled.
    #[serde(default)]
    pub enabled: Option<bool>,

    /// Maximum number of records to keep.
    #[serde(default)]
    pub max_records: Option<usize>,

    /// Maximum total size of all records.
    #[serde(default)]
    pub max_total_size: Option<ByteSize>,

    /// Maximum age of records.
    #[serde(default, with = "humantime_serde")]
    pub max_age: Option<Duration>,

    /// Maximum size of a single output (stdout/stderr) before truncation.
    #[serde(default)]
    pub max_output_size: Option<ByteSize>,
}

/// Default record configuration with all values required.
///
/// This is parsed from the embedded default user config TOML. All fields are
/// required - if the TOML is missing any field, parsing fails.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DefaultRecordConfig {
    /// Whether recording is enabled by default.
    pub enabled: bool,

    /// Maximum number of records to keep.
    pub max_records: usize,

    /// Maximum total size of all records.
    pub max_total_size: ByteSize,

    /// Maximum age of records.
    #[serde(with = "humantime_serde")]
    pub max_age: Duration,

    /// Maximum size of a single output (stdout/stderr) before truncation.
    pub max_output_size: ByteSize,
}

/// Deserialized form of record override settings.
///
/// Each field is optional; only the fields that are specified will override the
/// base configuration.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::user_config) struct DeserializedRecordOverrideData {
    /// Whether recording is enabled.
    pub(in crate::user_config) enabled: Option<bool>,

    /// Maximum number of records to keep.
    pub(in crate::user_config) max_records: Option<usize>,

    /// Maximum total size of all records.
    pub(in crate::user_config) max_total_size: Option<ByteSize>,

    /// Maximum age of records.
    #[serde(default, with = "humantime_serde")]
    pub(in crate::user_config) max_age: Option<Duration>,

    /// Maximum size of a single output (stdout/stderr) before truncation.
    pub(in crate::user_config) max_output_size: Option<ByteSize>,
}

/// A compiled record override with parsed platform spec.
///
/// This is created after parsing the platform expression from a
/// `[[overrides]]` entry.
#[derive(Clone, Debug)]
pub(in crate::user_config) struct CompiledRecordOverride {
    platform_spec: TargetSpec,
    data: RecordOverrideData,
}

impl CompiledRecordOverride {
    /// Creates a new compiled override from a platform spec and record data.
    pub(in crate::user_config) fn new(
        platform_spec: TargetSpec,
        data: DeserializedRecordOverrideData,
    ) -> Self {
        Self {
            platform_spec,
            data: RecordOverrideData {
                enabled: data.enabled,
                max_records: data.max_records,
                max_total_size: data.max_total_size,
                max_age: data.max_age,
                max_output_size: data.max_output_size,
            },
        }
    }

    /// Checks if this override matches the host platform.
    ///
    /// Unknown results (e.g., unrecognized target features) are treated as
    /// non-matching to be conservative.
    pub(in crate::user_config) fn matches(&self, host_platform: &Platform) -> bool {
        self.platform_spec
            .eval(host_platform)
            .unwrap_or(/* unknown results are mapped to false */ false)
    }

    /// Returns a reference to the override data.
    pub(in crate::user_config) fn data(&self) -> &RecordOverrideData {
        &self.data
    }
}

/// Override data for record settings.
#[derive(Clone, Debug, Default)]
pub(in crate::user_config) struct RecordOverrideData {
    enabled: Option<bool>,
    max_records: Option<usize>,
    max_total_size: Option<ByteSize>,
    max_age: Option<Duration>,
    max_output_size: Option<ByteSize>,
}

impl RecordOverrideData {
    /// Returns the enabled setting, if specified.
    pub(in crate::user_config) fn enabled(&self) -> Option<&bool> {
        self.enabled.as_ref()
    }

    /// Returns the max_records setting, if specified.
    pub(in crate::user_config) fn max_records(&self) -> Option<&usize> {
        self.max_records.as_ref()
    }

    /// Returns the max_total_size setting, if specified.
    pub(in crate::user_config) fn max_total_size(&self) -> Option<&ByteSize> {
        self.max_total_size.as_ref()
    }

    /// Returns the max_age setting, if specified.
    pub(in crate::user_config) fn max_age(&self) -> Option<&Duration> {
        self.max_age.as_ref()
    }

    /// Returns the max_output_size setting, if specified.
    pub(in crate::user_config) fn max_output_size(&self) -> Option<&ByteSize> {
        self.max_output_size.as_ref()
    }
}

/// Resolved record configuration after applying defaults.
#[derive(Clone, Debug)]
pub struct RecordConfig {
    /// Whether recording is enabled.
    ///
    /// Recording only occurs when both this is true and the `record`
    /// experimental feature is enabled.
    pub enabled: bool,

    /// Maximum number of records to keep.
    pub max_records: usize,

    /// Maximum total size of all records.
    pub max_total_size: ByteSize,

    /// Maximum age of records.
    pub max_age: Duration,

    /// Maximum size of a single output (stdout/stderr) before truncation.
    pub max_output_size: ByteSize,
}

impl RecordConfig {
    /// Resolves record configuration from user configs, defaults, and the host
    /// platform.
    ///
    /// Resolution order (highest to lowest priority):
    ///
    /// 1. User overrides (first matching override for each setting)
    /// 2. Default overrides (first matching override for each setting)
    /// 3. User base config
    /// 4. Default base config
    ///
    /// This matches the resolution order used by repo config.
    ///
    /// If `max_output_size` is below [`MIN_MAX_OUTPUT_SIZE`], it is clamped
    /// to the minimum and a warning is logged.
    pub(in crate::user_config) fn resolve(
        default_config: &DefaultRecordConfig,
        default_overrides: &[CompiledRecordOverride],
        user_config: Option<&DeserializedRecordConfig>,
        user_overrides: &[CompiledRecordOverride],
        host_platform: &Platform,
    ) -> Self {
        let mut max_output_size = resolve_record_setting(
            &default_config.max_output_size,
            default_overrides,
            user_config.and_then(|c| c.max_output_size.as_ref()),
            user_overrides,
            host_platform,
            |data| data.max_output_size(),
        );

        // Enforce minimum to ensure truncation marker fits.
        if max_output_size < MIN_MAX_OUTPUT_SIZE {
            tracing::warn!(
                "max-output-size ({}) is below minimum ({}), using minimum",
                max_output_size,
                MIN_MAX_OUTPUT_SIZE,
            );
            max_output_size = MIN_MAX_OUTPUT_SIZE;
        } else if max_output_size > MAX_MAX_OUTPUT_SIZE {
            tracing::warn!(
                "max-output-size ({}) exceeds maximum ({}), using maximum",
                max_output_size,
                MAX_MAX_OUTPUT_SIZE,
            );
            max_output_size = MAX_MAX_OUTPUT_SIZE;
        }

        Self {
            enabled: resolve_record_setting(
                &default_config.enabled,
                default_overrides,
                user_config.and_then(|c| c.enabled.as_ref()),
                user_overrides,
                host_platform,
                |data| data.enabled(),
            ),
            max_records: resolve_record_setting(
                &default_config.max_records,
                default_overrides,
                user_config.and_then(|c| c.max_records.as_ref()),
                user_overrides,
                host_platform,
                |data| data.max_records(),
            ),
            max_total_size: resolve_record_setting(
                &default_config.max_total_size,
                default_overrides,
                user_config.and_then(|c| c.max_total_size.as_ref()),
                user_overrides,
                host_platform,
                |data| data.max_total_size(),
            ),
            max_age: resolve_record_setting(
                &default_config.max_age,
                default_overrides,
                user_config.and_then(|c| c.max_age.as_ref()),
                user_overrides,
                host_platform,
                |data| data.max_age(),
            ),
            max_output_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::detect_host_platform_for_tests;

    #[test]
    fn test_deserialized_record_config_parsing() {
        // Test full config.
        let config: DeserializedRecordConfig = toml::from_str(
            r#"
            enabled = true
            max-records = 50
            max-total-size = "2GB"
            max-age = "7d"
            max-output-size = "5MB"
            "#,
        )
        .unwrap();

        assert_eq!(config.enabled, Some(true));
        assert_eq!(config.max_records, Some(50));
        assert_eq!(config.max_total_size, Some(ByteSize::gb(2)));
        assert_eq!(config.max_age, Some(Duration::from_secs(7 * 24 * 60 * 60)));
        assert_eq!(config.max_output_size, Some(ByteSize::mb(5)));

        // Test partial config.
        let config: DeserializedRecordConfig = toml::from_str(
            r#"
            max-records = 100
            "#,
        )
        .unwrap();

        assert!(config.enabled.is_none());
        assert_eq!(config.max_records, Some(100));
        assert!(config.max_total_size.is_none());
        assert!(config.max_age.is_none());
        assert!(config.max_output_size.is_none());

        // Test empty config.
        let config: DeserializedRecordConfig = toml::from_str("").unwrap();
        assert!(config.enabled.is_none());
        assert!(config.max_records.is_none());
        assert!(config.max_total_size.is_none());
        assert!(config.max_age.is_none());
        assert!(config.max_output_size.is_none());
    }

    #[test]
    fn test_default_record_config_parsing() {
        let config: DefaultRecordConfig = toml::from_str(
            r#"
            enabled = true
            max-records = 100
            max-total-size = "1GB"
            max-age = "30d"
            max-output-size = "10MB"
            "#,
        )
        .unwrap();

        assert!(config.enabled);
        assert_eq!(config.max_records, 100);
        assert_eq!(config.max_total_size, ByteSize::gb(1));
        assert_eq!(config.max_age, Duration::from_secs(30 * 24 * 60 * 60));
        assert_eq!(config.max_output_size, ByteSize::mb(10));
    }

    #[test]
    fn test_resolve_uses_defaults() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[], None, &[], &host);

        assert!(!resolved.enabled);
        assert_eq!(resolved.max_records, 100);
        assert_eq!(resolved.max_total_size, ByteSize::gb(1));
        assert_eq!(resolved.max_age, Duration::from_secs(30 * 24 * 60 * 60));
        assert_eq!(resolved.max_output_size, ByteSize::mb(10));
    }

    #[test]
    fn test_resolve_user_overrides_defaults() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        let user_config = DeserializedRecordConfig {
            enabled: Some(true),
            max_records: Some(50),
            max_total_size: None,
            max_age: Some(Duration::from_secs(7 * 24 * 60 * 60)),
            max_output_size: Some(ByteSize::mb(5)),
        };

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[], Some(&user_config), &[], &host);

        assert!(resolved.enabled); // From user.
        assert_eq!(resolved.max_records, 50); // From user.
        assert_eq!(resolved.max_total_size, ByteSize::gb(1)); // From defaults.
        assert_eq!(resolved.max_age, Duration::from_secs(7 * 24 * 60 * 60)); // From user.
        assert_eq!(resolved.max_output_size, ByteSize::mb(5)); // From user.
    }

    #[test]
    fn test_resolve_clamps_small_max_output_size() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // User specifies a value below the minimum.
        let user_config = DeserializedRecordConfig {
            enabled: None,
            max_records: None,
            max_total_size: None,
            max_age: None,
            max_output_size: Some(ByteSize::b(100)), // Way below minimum.
        };

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[], Some(&user_config), &[], &host);

        // Should be clamped to the minimum.
        assert_eq!(resolved.max_output_size, MIN_MAX_OUTPUT_SIZE);
    }

    #[test]
    fn test_resolve_accepts_value_at_minimum() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // User specifies exactly the minimum.
        let user_config = DeserializedRecordConfig {
            enabled: None,
            max_records: None,
            max_total_size: None,
            max_age: None,
            max_output_size: Some(MIN_MAX_OUTPUT_SIZE),
        };

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[], Some(&user_config), &[], &host);

        // Should be accepted as-is.
        assert_eq!(resolved.max_output_size, MIN_MAX_OUTPUT_SIZE);
    }

    #[test]
    fn test_resolve_clamps_large_max_output_size() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // User specifies a value above the maximum.
        let user_config = DeserializedRecordConfig {
            enabled: None,
            max_records: None,
            max_total_size: None,
            max_age: None,
            max_output_size: Some(ByteSize::gib(1)), // Way above maximum.
        };

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[], Some(&user_config), &[], &host);

        // Should be clamped to the maximum.
        assert_eq!(resolved.max_output_size, MAX_MAX_OUTPUT_SIZE);
    }

    #[test]
    fn test_resolve_accepts_value_at_maximum() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // User specifies exactly the maximum.
        let user_config = DeserializedRecordConfig {
            enabled: None,
            max_records: None,
            max_total_size: None,
            max_age: None,
            max_output_size: Some(MAX_MAX_OUTPUT_SIZE),
        };

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[], Some(&user_config), &[], &host);

        // Should be accepted as-is.
        assert_eq!(resolved.max_output_size, MAX_MAX_OUTPUT_SIZE);
    }

    /// Helper to create a CompiledRecordOverride for tests.
    fn make_override(
        platform: &str,
        data: DeserializedRecordOverrideData,
    ) -> CompiledRecordOverride {
        let platform_spec =
            TargetSpec::new(platform.to_string()).expect("valid platform spec in test");
        CompiledRecordOverride::new(platform_spec, data)
    }

    #[test]
    fn test_resolve_user_override_applies() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // Create a user override that matches any platform.
        let override_ = make_override(
            "cfg(all())",
            DeserializedRecordOverrideData {
                enabled: Some(true),
                max_records: Some(50),
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[], None, &[override_], &host);

        assert!(resolved.enabled);
        assert_eq!(resolved.max_records, 50);
        assert_eq!(resolved.max_total_size, ByteSize::gb(1)); // From defaults.
        assert_eq!(resolved.max_age, Duration::from_secs(30 * 24 * 60 * 60)); // From defaults.
        assert_eq!(resolved.max_output_size, ByteSize::mb(10)); // From defaults.
    }

    #[test]
    fn test_resolve_default_override_applies() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // Create a default override that matches any platform.
        let override_ = make_override(
            "cfg(all())",
            DeserializedRecordOverrideData {
                enabled: Some(true),
                max_records: Some(50),
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[override_], None, &[], &host);

        assert!(resolved.enabled);
        assert_eq!(resolved.max_records, 50);
        assert_eq!(resolved.max_total_size, ByteSize::gb(1)); // From defaults.
    }

    #[test]
    fn test_resolve_platform_override_no_match() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // Create an override that never matches (cfg(any()) with no arguments
        // is false).
        let override_ = make_override(
            "cfg(any())",
            DeserializedRecordOverrideData {
                enabled: Some(true),
                max_records: Some(50),
                max_total_size: Some(ByteSize::gb(2)),
                max_age: Some(Duration::from_secs(7 * 24 * 60 * 60)),
                max_output_size: Some(ByteSize::mb(5)),
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[], None, &[override_], &host);

        // Nothing should be overridden - all values should match defaults.
        assert!(!resolved.enabled);
        assert_eq!(resolved.max_records, 100);
        assert_eq!(resolved.max_total_size, ByteSize::gb(1));
        assert_eq!(resolved.max_age, Duration::from_secs(30 * 24 * 60 * 60));
        assert_eq!(resolved.max_output_size, ByteSize::mb(10));
    }

    #[test]
    fn test_resolve_first_matching_user_override_wins() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // Create two user overrides that both match (cfg(all()) is always true).
        let override1 = make_override(
            "cfg(all())",
            DeserializedRecordOverrideData {
                enabled: Some(true),
                ..Default::default()
            },
        );

        let override2 = make_override(
            "cfg(all())",
            DeserializedRecordOverrideData {
                enabled: Some(false), // Should be ignored.
                max_records: Some(50),
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[], None, &[override1, override2], &host);

        // First override wins for enabled.
        assert!(resolved.enabled);
        // Second override's max_records applies (first didn't set it).
        assert_eq!(resolved.max_records, 50);
    }

    #[test]
    fn test_resolve_user_override_beats_default_override() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // User override sets enabled.
        let user_override = make_override(
            "cfg(all())",
            DeserializedRecordOverrideData {
                enabled: Some(true),
                ..Default::default()
            },
        );

        // Default override sets enabled and max_records.
        let default_override = make_override(
            "cfg(all())",
            DeserializedRecordOverrideData {
                enabled: Some(false), // Should be ignored.
                max_records: Some(50),
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(
            &defaults,
            &[default_override],
            None,
            &[user_override],
            &host,
        );

        // User override wins for enabled.
        assert!(resolved.enabled);
        // Default override applies for max_records (user didn't set it).
        assert_eq!(resolved.max_records, 50);
    }

    #[test]
    fn test_resolve_override_beats_user_base() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // User base config sets enabled.
        let user_config = DeserializedRecordConfig {
            enabled: Some(false),
            max_records: Some(25),
            ..Default::default()
        };

        // Default override sets enabled (should beat user base).
        let default_override = make_override(
            "cfg(all())",
            DeserializedRecordOverrideData {
                enabled: Some(true),
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(
            &defaults,
            &[default_override],
            Some(&user_config),
            &[],
            &host,
        );

        // Default override is chosen over user base for enabled.
        assert!(resolved.enabled);
        // User base applies for max_records (override didn't set it).
        assert_eq!(resolved.max_records, 25);
    }

    #[test]
    fn test_resolve_override_clamps_max_output_size() {
        let defaults = DefaultRecordConfig {
            enabled: false,
            max_records: 100,
            max_total_size: ByteSize::gb(1),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        // Override specifies a value below the minimum.
        let override_ = make_override(
            "cfg(all())",
            DeserializedRecordOverrideData {
                max_output_size: Some(ByteSize::b(100)), // Way below minimum.
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = RecordConfig::resolve(&defaults, &[], None, &[override_], &host);

        // Should be clamped to the minimum.
        assert_eq!(resolved.max_output_size, MIN_MAX_OUTPUT_SIZE);
    }
}
