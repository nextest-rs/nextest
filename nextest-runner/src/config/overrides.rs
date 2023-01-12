// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::NextestConfigImpl;
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

/// Override settings for individual tests.
///
/// Returned by [`NextestProfile::overrides_for`].
///
/// The `Source` parameter tracks an optional source; this isn't used by any public APIs at the
/// moment.
#[derive(Clone, Debug)]
pub struct ProfileOverrides<Source = ()> {
    threads_required: Option<(ThreadsRequired, Source)>,
    retries: Option<(RetryPolicy, Source)>,
    slow_timeout: Option<(SlowTimeout, Source)>,
    leak_timeout: Option<(Duration, Source)>,
    test_group: Option<(TestGroup, Source)>,
}

pub(crate) trait OverrideSource<'p>: Sized {
    fn track_source<T>(
        value: Option<T>,
        source: &'p CompiledOverride<FinalConfig>,
    ) -> Option<(T, Self)>;
}

impl<'p> OverrideSource<'p> for () {
    fn track_source<T>(
        value: Option<T>,
        _source: &'p CompiledOverride<FinalConfig>,
    ) -> Option<(T, Self)> {
        value.map(|value| (value, ()))
    }
}

impl<'p> OverrideSource<'p> for &'p CompiledOverride<FinalConfig> {
    fn track_source<T>(
        value: Option<T>,
        source: &'p CompiledOverride<FinalConfig>,
    ) -> Option<(T, Self)> {
        value.map(|value| (value, source))
    }
}

impl ProfileOverrides {
    /// Returns the number of threads required for this test.
    pub fn threads_required(&self) -> Option<ThreadsRequired> {
        self.threads_required.map(|x| x.0)
    }

    /// Returns the number of retries for this test.
    pub fn retries(&self) -> Option<RetryPolicy> {
        self.retries.map(|x| x.0)
    }

    /// Returns the slow timeout for this test.
    pub fn slow_timeout(&self) -> Option<SlowTimeout> {
        self.slow_timeout.map(|x| x.0)
    }

    /// Returns the leak timeout for this test.
    pub fn leak_timeout(&self) -> Option<Duration> {
        self.leak_timeout.map(|x| x.0)
    }

    /// Returns the test group for this test.
    pub fn test_group(&self) -> Option<&TestGroup> {
        self.test_group.as_ref().map(|x| &x.0)
    }
}

#[allow(dead_code)]
impl<Source> ProfileOverrides<Source> {
    pub(super) fn new<'p>(
        overrides: &'p [CompiledOverride<FinalConfig>],
        query: &TestQuery<'_>,
    ) -> Self
    where
        Source: OverrideSource<'p>,
    {
        let mut threads_required = None;
        let mut retries = None;
        let mut slow_timeout = None;
        let mut leak_timeout = None;
        let mut test_group = None;

        for override_ in overrides {
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
            if threads_required.is_none() && override_.data.threads_required.is_some() {
                threads_required = Source::track_source(override_.data.threads_required, override_);
            }
            if retries.is_none() && override_.data.retries.is_some() {
                retries = Source::track_source(override_.data.retries, override_);
            }
            if slow_timeout.is_none() && override_.data.slow_timeout.is_some() {
                slow_timeout = Source::track_source(override_.data.slow_timeout, override_);
            }
            if leak_timeout.is_none() && override_.data.leak_timeout.is_some() {
                leak_timeout = Source::track_source(override_.data.leak_timeout, override_);
            }
            if test_group.is_none() && override_.data.test_group.is_some() {
                test_group = Source::track_source(override_.data.test_group.clone(), override_);
            }
        }

        ProfileOverrides {
            threads_required,
            retries,
            slow_timeout,
            leak_timeout,
            test_group,
        }
    }

    /// Returns the number of threads required for this test, with the source attached.
    pub(crate) fn threads_required_with_source(&self) -> Option<(ThreadsRequired, &Source)> {
        self.threads_required.as_ref().map(|x| (x.0, &x.1))
    }

    /// Returns the number of retries for this test, with the source attached.
    pub(crate) fn retries_with_source(&self) -> Option<(RetryPolicy, &Source)> {
        self.retries.as_ref().map(|x| (x.0, &x.1))
    }

    /// Returns the slow timeout for this test, with the source attached.
    pub(crate) fn slow_timeout_with_source(&self) -> Option<(SlowTimeout, &Source)> {
        self.slow_timeout.as_ref().map(|x| (x.0, &x.1))
    }

    /// Returns the leak timeout for this test, with the source attached.
    pub(crate) fn leak_timeout_with_source(&self) -> Option<(Duration, &Source)> {
        self.leak_timeout.as_ref().map(|x| (x.0, &x.1))
    }

    /// Returns the test group for this test, with the source attached.
    pub(crate) fn test_group_with_source(&self) -> Option<(&TestGroup, &Source)> {
        self.test_group.as_ref().map(|x| (&x.0, &x.1))
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
        source: &ProfileOverrideSource,
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

/// Pre-compiled form of profile overrides.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ProfileOverrideSource {
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

#[derive(Clone, Debug, Default)]
pub(super) struct NextestOverridesImpl {
    pub(super) default: Vec<CompiledOverride<PreBuildPlatform>>,
    pub(super) other: HashMap<String, Vec<CompiledOverride<PreBuildPlatform>>>,
}

impl NextestOverridesImpl {
    pub(super) fn new(
        graph: &PackageGraph,
        config: &NextestConfigImpl,
    ) -> Result<Self, ConfigParseErrorKind> {
        let mut errors = vec![];
        let default = Self::compile_overrides(
            graph,
            "default",
            &config.default_profile.overrides,
            &mut errors,
        );
        let other: HashMap<_, _> = config
            .other_profiles
            .iter()
            .map(|(profile_name, profile)| {
                (
                    profile_name.clone(),
                    Self::compile_overrides(graph, profile_name, &profile.overrides, &mut errors),
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
        overrides: &[ProfileOverrideSource],
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
        let overrides = profile.overrides_for(&query);

        assert_eq!(
            overrides.threads_required(),
            Some(ThreadsRequired::Count(8))
        );
        assert_eq!(overrides.retries(), Some(RetryPolicy::new_without_delay(3)));
        assert_eq!(
            overrides.slow_timeout(),
            Some(SlowTimeout {
                period: Duration::from_secs(60),
                terminate_after: None,
                grace_period: Duration::from_secs(10),
            })
        );
        assert_eq!(overrides.leak_timeout(), Some(Duration::from_millis(300)));
        assert_eq!(overrides.test_group(), Some(&test_group("my-group")));

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
        let overrides = profile.overrides_for(&query);

        assert_eq!(
            overrides.threads_required(),
            Some(ThreadsRequired::Count(8))
        );
        assert_eq!(
            overrides.retries(),
            Some(RetryPolicy::Exponential {
                count: 20,
                delay: Duration::from_secs(1),
                jitter: false,
                max_delay: Some(Duration::from_secs(20)),
            })
        );
        assert_eq!(
            overrides.slow_timeout(),
            Some(SlowTimeout {
                period: Duration::from_secs(120),
                terminate_after: Some(NonZeroUsize::new(1).unwrap()),
                grace_period: Duration::ZERO,
            })
        );
        assert_eq!(overrides.leak_timeout(), Some(Duration::from_millis(300)));
        assert_eq!(overrides.test_group(), Some(&test_group("my-group")));
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
