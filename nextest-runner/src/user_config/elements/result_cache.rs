// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Result-cache user configuration.

use crate::errors::UserConfigError;
use chrono::TimeDelta;
use serde::Deserialize;
use std::time::Duration;

/// User-provided `[result-cache]` settings. All fields are optional and fall
/// back to the embedded defaults.
#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(feature = "config-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "config-schema", schemars(deny_unknown_fields))]
#[serde(rename_all = "kebab-case")]
pub struct DeserializedResultCacheConfig {
    /// How long a binary's cached results are kept after it was last used
    /// (e.g. `"7d"`).
    #[serde(default, with = "humantime_serde")]
    #[cfg_attr(feature = "config-schema", schemars(with = "Option<String>"))]
    pub prune_grace: Option<Duration>,

    /// Minimum time between automatic prunes (e.g. `"1d"`).
    #[serde(default, with = "humantime_serde")]
    #[cfg_attr(feature = "config-schema", schemars(with = "Option<String>"))]
    pub prune_interval: Option<Duration>,
}

/// Default `[result-cache]` config, parsed from the embedded default user
/// config TOML. All fields are required.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DefaultResultCacheConfig {
    /// How long a binary's cached results are kept after it was last used.
    #[serde(with = "humantime_serde")]
    pub prune_grace: Duration,

    /// Minimum time between automatic prunes.
    #[serde(with = "humantime_serde")]
    pub prune_interval: Duration,
}

/// Resolved result-cache configuration after applying defaults.
///
/// Durations are stored as [`TimeDelta`], validated during [`resolve`](Self::resolve).
#[derive(Clone, Debug)]
pub struct ResultCacheConfig {
    /// How long a binary's cached results are kept after it was last used.
    ///
    /// Long enough that editing a file and reverting it (which recompiles back
    /// to the old binary hash) still finds the old results; beyond it, results
    /// for binaries that have gone untouched are pruned.
    pub prune_grace: TimeDelta,

    /// Minimum time between automatic prunes.
    ///
    /// Pruning walks every cache directory, which is cheap but pointless to
    /// repeat on every run, so it is rate-limited to this interval.
    pub prune_interval: TimeDelta,
}

impl ResultCacheConfig {
    /// Resolves result-cache configuration from the user config layered over the
    /// embedded defaults.
    ///
    /// Errors if a configured duration is out of [`TimeDelta`]'s supported
    /// range, so an invalid config fails the run rather than being silently
    /// clamped.
    pub(in crate::user_config) fn resolve(
        default_config: &DefaultResultCacheConfig,
        user_config: Option<&DeserializedResultCacheConfig>,
    ) -> Result<Self, UserConfigError> {
        let prune_grace = user_config
            .and_then(|c| c.prune_grace)
            .unwrap_or(default_config.prune_grace);
        let prune_interval = user_config
            .and_then(|c| c.prune_interval)
            .unwrap_or(default_config.prune_interval);

        Ok(Self {
            prune_grace: to_time_delta("prune-grace", prune_grace)?,
            prune_interval: to_time_delta("prune-interval", prune_interval)?,
        })
    }
}

/// Converts a [`Duration`] to a [`TimeDelta`], erroring if it is out of range.
fn to_time_delta(field: &'static str, value: Duration) -> Result<TimeDelta, UserConfigError> {
    TimeDelta::from_std(value).map_err(|_| UserConfigError::ResultCacheDuration { field, value })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arbitrary fixture values, deliberately *not* the real embedded defaults,
    /// so the tests exercise resolution rather than coincidentally matching the
    /// shipped config.
    const FIXTURE_GRACE: Duration = Duration::from_secs(11 * 60 * 60);
    const FIXTURE_INTERVAL: Duration = Duration::from_secs(3 * 60 * 60);

    fn defaults() -> DefaultResultCacheConfig {
        DefaultResultCacheConfig {
            prune_grace: FIXTURE_GRACE,
            prune_interval: FIXTURE_INTERVAL,
        }
    }

    #[test]
    fn resolve_uses_defaults_when_unset() {
        let resolved = ResultCacheConfig::resolve(&defaults(), None).unwrap();
        assert_eq!(resolved.prune_grace, TimeDelta::hours(11));
        assert_eq!(resolved.prune_interval, TimeDelta::hours(3));
    }

    #[test]
    fn resolve_user_overrides_defaults_per_field() {
        // Only prune-grace is set by the user; prune-interval falls back.
        let user = DeserializedResultCacheConfig {
            prune_grace: Some(Duration::from_secs(60 * 60)),
            prune_interval: None,
        };
        let resolved = ResultCacheConfig::resolve(&defaults(), Some(&user)).unwrap();
        assert_eq!(resolved.prune_grace, TimeDelta::hours(1));
        assert_eq!(resolved.prune_interval, TimeDelta::hours(3));
    }

    #[test]
    fn resolve_errors_on_out_of_range_duration() {
        let user = DeserializedResultCacheConfig {
            prune_grace: Some(Duration::MAX),
            prune_interval: None,
        };
        let error = ResultCacheConfig::resolve(&defaults(), Some(&user)).unwrap_err();
        assert!(
            matches!(
                error,
                UserConfigError::ResultCacheDuration {
                    field: "prune-grace",
                    ..
                }
            ),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn deserialize_parses_humantime_durations() {
        let config: DeserializedResultCacheConfig = toml::from_str(
            r#"
            prune-grace = "3d"
            prune-interval = "12h"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.prune_grace,
            Some(Duration::from_secs(3 * 24 * 60 * 60))
        );
        assert_eq!(
            config.prune_interval,
            Some(Duration::from_secs(12 * 60 * 60))
        );
    }
}
