// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{NextestConfigImpl, NextestProfile};
use crate::{
    config::{FinalConfig, PreBuildPlatform, RetryPolicy, SlowTimeout, TestGroup, ThreadsRequired},
    errors::{ConfigParseErrorKind, ConfigParseOverrideError},
    platform::BuildPlatforms,
};
use guppy::graph::{cargo::BuildPlatform, PackageGraph};
use nextest_filtering::{FilteringExpr, TestQuery};
use serde::Deserialize;
use smol_str::SmolStr;
use std::{collections::HashMap, time::Duration};
use target_spec::TargetSpec;

/// Settings for individual tests.
///
/// Returned by [`NextestProfile::settings_for`].
///
/// The `Source` parameter tracks an optional source; this isn't used by any public APIs at the
/// moment.
#[derive(Clone, Debug)]
pub struct TestSettings<Source = ()> {
    threads_required: (ThreadsRequired, Source),
    retries: (RetryPolicy, Source),
    slow_timeout: (SlowTimeout, Source),
    leak_timeout: (Duration, Source),
    test_group: (TestGroup, Source),
}

pub(crate) trait TrackSource<'p>: Sized {
    fn track_profile<T>(value: T) -> (T, Self);
    fn track_override<T>(value: T, source: &'p CompiledOverride<FinalConfig>) -> (T, Self);
}

impl<'p> TrackSource<'p> for () {
    fn track_profile<T>(value: T) -> (T, Self) {
        (value, ())
    }

    fn track_override<T>(value: T, _source: &'p CompiledOverride<FinalConfig>) -> (T, Self) {
        (value, ())
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum SettingSource<'p> {
    Profile,
    Override(&'p CompiledOverride<FinalConfig>),
}

impl<'p> TrackSource<'p> for SettingSource<'p> {
    fn track_profile<T>(value: T) -> (T, Self) {
        (value, SettingSource::Profile)
    }

    fn track_override<T>(value: T, source: &'p CompiledOverride<FinalConfig>) -> (T, Self) {
        (value, SettingSource::Override(source))
    }
}

impl TestSettings {
    /// Returns the number of threads required for this test.
    pub fn threads_required(&self) -> ThreadsRequired {
        self.threads_required.0
    }

    /// Returns the number of retries for this test.
    pub fn retries(&self) -> RetryPolicy {
        self.retries.0
    }

    /// Returns the slow timeout for this test.
    pub fn slow_timeout(&self) -> SlowTimeout {
        self.slow_timeout.0
    }

    /// Returns the leak timeout for this test.
    pub fn leak_timeout(&self) -> Duration {
        self.leak_timeout.0
    }

    /// Returns the test group for this test.
    pub fn test_group(&self) -> &TestGroup {
        &self.test_group.0
    }
}

#[allow(dead_code)]
impl<Source: Copy> TestSettings<Source> {
    pub(super) fn new<'p>(
        profile: &'p NextestProfile<'_, FinalConfig>,
        query: &TestQuery<'_>,
    ) -> Self
    where
        Source: TrackSource<'p>,
    {
        let mut threads_required = None;
        let mut retries = None;
        let mut slow_timeout = None;
        let mut leak_timeout = None;
        let mut test_group = None;

        for override_ in &profile.overrides {
            if query.binary_query.platform == BuildPlatform::Host && !override_.state.host_eval {
                continue;
            }
            if query.binary_query.platform == BuildPlatform::Target && !override_.state.target_eval
            {
                continue;
            }

            if let Some((_, expr)) = &override_.data.expr {
                if !expr.matches_test(query) {
                    continue;
                }
                // If no expression is present, it's equivalent to "all()".
            }
            if threads_required.is_none() {
                if let Some(t) = override_.data.threads_required {
                    threads_required = Some(Source::track_override(t, override_));
                }
            }
            if retries.is_none() {
                if let Some(r) = override_.data.retries {
                    retries = Some(Source::track_override(r, override_));
                }
            }
            if slow_timeout.is_none() {
                if let Some(s) = override_.data.slow_timeout {
                    slow_timeout = Some(Source::track_override(s, override_));
                }
            }
            if leak_timeout.is_none() {
                if let Some(l) = override_.data.leak_timeout {
                    leak_timeout = Some(Source::track_override(l, override_));
                }
            }
            if test_group.is_none() {
                if let Some(t) = &override_.data.test_group {
                    test_group = Some(Source::track_override(t.clone(), override_));
                }
            }
        }

        // If no overrides were found, use the profile defaults.
        let threads_required =
            threads_required.unwrap_or_else(|| Source::track_profile(profile.threads_required()));
        let retries = retries.unwrap_or_else(|| Source::track_profile(profile.retries()));
        let slow_timeout =
            slow_timeout.unwrap_or_else(|| Source::track_profile(profile.slow_timeout()));
        let leak_timeout =
            leak_timeout.unwrap_or_else(|| Source::track_profile(profile.leak_timeout()));
        let test_group = test_group.unwrap_or_else(|| Source::track_profile(TestGroup::Global));

        TestSettings {
            threads_required,
            retries,
            slow_timeout,
            leak_timeout,
            test_group,
        }
    }

    /// Returns the number of threads required for this test, with the source attached.
    pub(crate) fn threads_required_with_source(&self) -> (ThreadsRequired, Source) {
        self.threads_required
    }

    /// Returns the number of retries for this test, with the source attached.
    pub(crate) fn retries_with_source(&self) -> (RetryPolicy, Source) {
        self.retries
    }

    /// Returns the slow timeout for this test, with the source attached.
    pub(crate) fn slow_timeout_with_source(&self) -> (SlowTimeout, Source) {
        self.slow_timeout
    }

    /// Returns the leak timeout for this test, with the source attached.
    pub(crate) fn leak_timeout_with_source(&self) -> (Duration, Source) {
        self.leak_timeout
    }

    /// Returns the test group for this test, with the source attached.
    pub(crate) fn test_group_with_source(&self) -> &(TestGroup, Source) {
        &self.test_group
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct CompiledOverridesByProfile {
    pub(super) default: Vec<CompiledOverride<PreBuildPlatform>>,
    pub(super) other: HashMap<String, Vec<CompiledOverride<PreBuildPlatform>>>,
}

impl CompiledOverridesByProfile {
    pub(super) fn new(
        graph: &PackageGraph,
        config: &NextestConfigImpl,
    ) -> Result<Self, ConfigParseErrorKind> {
        let mut errors = vec![];
        let default = Self::compile_overrides(
            graph,
            "default",
            config.default_profile().overrides(),
            &mut errors,
        );
        let other: HashMap<_, _> = config
            .other_profiles()
            .map(|(profile_name, profile)| {
                (
                    profile_name.to_owned(),
                    Self::compile_overrides(graph, profile_name, profile.overrides(), &mut errors),
                )
            })
            .collect();

        if errors.is_empty() {
            Ok(Self { default, other })
        } else {
            Err(ConfigParseErrorKind::OverrideError(errors))
        }
    }

    fn compile_overrides(
        graph: &PackageGraph,
        profile_name: &str,
        overrides: &[DeserializedOverride],
        errors: &mut Vec<ConfigParseOverrideError>,
    ) -> Vec<CompiledOverride<PreBuildPlatform>> {
        overrides
            .iter()
            .enumerate()
            .filter_map(|(index, source)| {
                CompiledOverride::new(graph, profile_name, index, source, errors)
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledOverride<State> {
    id: OverrideId,
    state: State,
    pub(super) data: ProfileOverrideData,
}

impl<State> CompiledOverride<State> {
    pub(crate) fn id(&self) -> &OverrideId {
        &self.id
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct OverrideId {
    pub(crate) profile_name: SmolStr,
    index: usize,
}

#[derive(Clone, Debug)]
pub(super) struct ProfileOverrideData {
    target_spec: Option<TargetSpec>,
    expr: Option<(String, FilteringExpr)>,
    threads_required: Option<ThreadsRequired>,
    retries: Option<RetryPolicy>,
    slow_timeout: Option<SlowTimeout>,
    leak_timeout: Option<Duration>,
    pub(super) test_group: Option<TestGroup>,
}

impl CompiledOverride<PreBuildPlatform> {
    fn new(
        graph: &PackageGraph,
        profile_name: &str,
        index: usize,
        source: &DeserializedOverride,
        errors: &mut Vec<ConfigParseOverrideError>,
    ) -> Option<Self> {
        if source.platform.is_none() && source.filter.is_none() {
            errors.push(ConfigParseOverrideError {
                profile_name: profile_name.to_owned(),
                not_specified: true,
                platform_parse_error: None,
                parse_errors: None,
            });
            return None;
        }

        let target_spec = source
            .platform
            .as_ref()
            .map(|platform_str| TargetSpec::new(platform_str.to_owned()))
            .transpose();
        let filter_expr = source.filter.as_ref().map_or(Ok(None), |filter| {
            Some(FilteringExpr::parse(filter, graph).map(|expr| (filter.clone(), expr))).transpose()
        });

        match (target_spec, filter_expr) {
            (Ok(target_spec), Ok(expr)) => Some(Self {
                id: OverrideId {
                    profile_name: profile_name.into(),
                    index,
                },
                state: PreBuildPlatform {},
                data: ProfileOverrideData {
                    target_spec,
                    expr,
                    threads_required: source.threads_required,
                    retries: source.retries,
                    slow_timeout: source.slow_timeout,
                    leak_timeout: source.leak_timeout,
                    test_group: source.test_group.clone(),
                },
            }),
            (Err(platform_parse_error), Ok(_)) => {
                errors.push(ConfigParseOverrideError {
                    profile_name: profile_name.to_owned(),
                    not_specified: false,
                    platform_parse_error: Some(platform_parse_error),
                    parse_errors: None,
                });
                None
            }
            (Ok(_), Err(parse_errors)) => {
                errors.push(ConfigParseOverrideError {
                    profile_name: profile_name.to_owned(),
                    not_specified: false,
                    platform_parse_error: None,
                    parse_errors: Some(parse_errors),
                });
                None
            }
            (Err(platform_parse_error), Err(parse_errors)) => {
                errors.push(ConfigParseOverrideError {
                    profile_name: profile_name.to_owned(),
                    not_specified: false,
                    platform_parse_error: Some(platform_parse_error),
                    parse_errors: Some(parse_errors),
                });
                None
            }
        }
    }

    pub(super) fn apply_build_platforms(
        self,
        build_platforms: &BuildPlatforms,
    ) -> CompiledOverride<FinalConfig> {
        let (host_eval, target_eval) = if let Some(spec) = &self.data.target_spec {
            // unknown (None) gets unwrapped to true.
            let host_eval = spec.eval(&build_platforms.host).unwrap_or(true);
            let target_eval = build_platforms.target.as_ref().map_or(host_eval, |triple| {
                spec.eval(&triple.platform).unwrap_or(true)
            });
            (host_eval, target_eval)
        } else {
            (true, true)
        };
        CompiledOverride {
            id: self.id,
            state: FinalConfig {
                host_eval,
                target_eval,
            },
            data: self.data,
        }
    }
}

impl CompiledOverride<FinalConfig> {
    /// Returns the target spec.
    pub(crate) fn target_spec(&self) -> Option<&TargetSpec> {
        self.data.target_spec.as_ref()
    }

    /// Returns the filter string and expression, if any.
    pub(crate) fn filter(&self) -> Option<(&str, &FilteringExpr)> {
        self.data
            .expr
            .as_ref()
            .map(|(filter, expr)| (filter.as_str(), expr))
    }
}

/// Deserialized form of profile overrides before compilation.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct DeserializedOverride {
    /// The platforms to match against.
    #[serde(default)]
    platform: Option<String>,
    /// The filter expression to match against.
    #[serde(default)]
    filter: Option<String>,
    /// Overrides. (This used to use serde(flatten) but that has issues:
    /// https://github.com/serde-rs/serde/issues/2312.)
    #[serde(default)]
    threads_required: Option<ThreadsRequired>,
    #[serde(default, deserialize_with = "super::deserialize_retry_policy")]
    retries: Option<RetryPolicy>,
    #[serde(default, deserialize_with = "super::deserialize_slow_timeout")]
    slow_timeout: Option<SlowTimeout>,
    #[serde(default, with = "humantime_serde::option")]
    leak_timeout: Option<Duration>,
    #[serde(default)]
    test_group: Option<TestGroup>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{test_helpers::*, NextestConfig};
    use camino::Utf8Path;
    use indoc::indoc;
    use nextest_filtering::BinaryQuery;
    use std::num::NonZeroUsize;
    use tempfile::tempdir;
    use test_case::test_case;

    /// Basic test to ensure overrides work. Add new override parameters to this test.
    #[test]
    fn test_overrides_basic() {
        let config_contents = indoc! {r#"
            [[profile.default.overrides]]
            platform = 'aarch64-apple-darwin'  # this is the target platform
            filter = "test(test)"
            retries = { backoff = "exponential", count = 20, delay = "1s", max-delay = "20s" }
            slow-timeout = { period = "120s", terminate-after = 1, grace-period = "0s" }

            [[profile.default.overrides]]
            filter = "test(test)"
            threads-required = 8
            retries = 3
            slow-timeout = "60s"
            leak-timeout = "300ms"
            test-group = "my-group"

            [test-groups.my-group]
            max-threads = 20
        "#};

        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);
        let package_id = graph.workspace().iter().next().unwrap().id();

        let nextest_config_result =
            NextestConfig::from_sources(graph.workspace().root(), &graph, None, &[][..])
                .expect("config is valid");
        let profile = nextest_config_result
            .profile("default")
            .expect("valid profile name")
            .apply_build_platforms(&build_platforms());

        // This query matches the second override.
        let query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: BuildPlatform::Host,
            },
            test_name: "test",
        };
        let overrides = profile.settings_for(&query);

        assert_eq!(overrides.threads_required(), ThreadsRequired::Count(8));
        assert_eq!(overrides.retries(), RetryPolicy::new_without_delay(3));
        assert_eq!(
            overrides.slow_timeout(),
            SlowTimeout {
                period: Duration::from_secs(60),
                terminate_after: None,
                grace_period: Duration::from_secs(10),
            }
        );
        assert_eq!(overrides.leak_timeout(), Duration::from_millis(300));
        assert_eq!(overrides.test_group(), &test_group("my-group"));

        // This query matches both overrides.
        let query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: BuildPlatform::Target,
            },
            test_name: "test",
        };
        let overrides = profile.settings_for(&query);

        assert_eq!(overrides.threads_required(), ThreadsRequired::Count(8));
        assert_eq!(
            overrides.retries(),
            RetryPolicy::Exponential {
                count: 20,
                delay: Duration::from_secs(1),
                jitter: false,
                max_delay: Some(Duration::from_secs(20)),
            }
        );
        assert_eq!(
            overrides.slow_timeout(),
            SlowTimeout {
                period: Duration::from_secs(120),
                terminate_after: Some(NonZeroUsize::new(1).unwrap()),
                grace_period: Duration::ZERO,
            }
        );
        assert_eq!(overrides.leak_timeout(), Duration::from_millis(300));
        assert_eq!(overrides.test_group(), &test_group("my-group"));
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    struct MietteJsonReport {
        message: String,
        labels: Vec<MietteJsonLabel>,
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    struct MietteJsonLabel {
        label: String,
        span: MietteJsonSpan,
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    struct MietteJsonSpan {
        offset: usize,
        length: usize,
    }

    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            retries = 2
        "#},
        "default",
        &[MietteJsonReport {
            message: "at least one of `platform` and `filter` should be specified".to_owned(),
            labels: vec![],
        }]

        ; "neither platform nor filter specified"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.ci.overrides]]
            platform = 'cfg(target_os = "macos)'
            retries = 2
        "#},
        "ci",
        &[MietteJsonReport {
            message: "error parsing cfg() expression".to_owned(),
            labels: vec![
                MietteJsonLabel { label: "unclosed quotes".to_owned(), span: MietteJsonSpan { offset: 16, length: 6 } }
            ]
        }]

        ; "invalid platform expression"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.ci.overrides]]
            filter = 'test(/foo)'
            retries = 2
        "#},
        "ci",
        &[MietteJsonReport {
            message: "expected close regex".to_owned(),
            labels: vec![
                MietteJsonLabel { label: "missing `/`".to_owned(), span: MietteJsonSpan { offset: 9, length: 0 } }
            ]
        }]

        ; "invalid filter expression"
    )]
    fn parse_overrides_invalid(
        config_contents: &str,
        faulty_profile: &str,
        expected_reports: &[MietteJsonReport],
    ) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);

        let err = NextestConfig::from_sources(graph.workspace().root(), &graph, None, [])
            .expect_err("config is invalid");
        match err.kind() {
            ConfigParseErrorKind::OverrideError(override_errors) => {
                assert_eq!(
                    override_errors.len(),
                    1,
                    "exactly one override error must be produced"
                );
                let error = override_errors.first().unwrap();
                assert_eq!(
                    error.profile_name, faulty_profile,
                    "override error profile matches"
                );
                let handler = miette::JSONReportHandler::new();
                let reports = error
                    .reports()
                    .map(|report| {
                        let mut out = String::new();
                        handler.render_report(&mut out, report.as_ref()).unwrap();

                        let json_report: MietteJsonReport = serde_json::from_str(&out)
                            .unwrap_or_else(|err| {
                                panic!(
                                    "failed to deserialize JSON message produced by miette: {err}"
                                )
                            });
                        json_report
                    })
                    .collect::<Vec<_>>();
                assert_eq!(&reports, expected_reports, "reports match");
            }
            other => {
                panic!("for config error {other:?}, expected ConfigParseErrorKind::OverrideError");
            }
        };
    }
}
