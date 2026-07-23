// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::config::elements::ReportSkipPolicy;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// Controls how flaky-fail tests are reported in JUnit XML output.
///
/// Flaky-fail tests are tests that eventually passed on retry but are configured
/// with `flaky-result = "fail"`. This setting controls whether they appear as
/// failures or successes in the JUnit report.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "config-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum JunitFlakyFailStatus {
    /// Report flaky-fail tests as failures with `<failure>` and
    /// `<flakyFailure>` elements.
    #[default]
    Failure,

    /// Report flaky-fail tests as successes, identical to flaky-pass tests.
    Success,
}

/// Global JUnit configuration stored within a profile.
///
/// Returned by an [`EvaluatableProfile`](crate::config::core::EvaluatableProfile).
#[derive(Clone, Debug)]
pub struct JunitConfig<'cfg> {
    path: Utf8PathBuf,
    report_name: &'cfg str,
    store_success_output: bool,
    store_failure_output: bool,
    report_skipped: ReportSkipPolicy,
    flaky_fail_status: JunitFlakyFailStatus,
}

impl<'cfg> JunitConfig<'cfg> {
    pub(in crate::config) fn new(
        store_dir: &Utf8Path,
        settings: JunitSettings<'cfg>,
    ) -> Option<Self> {
        let path = settings.path?;
        Some(Self {
            path: store_dir.join(path),
            report_name: settings.report_name,
            store_success_output: settings.store_success_output,
            store_failure_output: settings.store_failure_output,
            report_skipped: settings.report_skipped,
            flaky_fail_status: settings.flaky_fail_status,
        })
    }

    /// Returns the absolute path to the JUnit report.
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// Returns the name of the JUnit report.
    pub fn report_name(&self) -> &'cfg str {
        self.report_name
    }

    /// Returns true if success output should be stored.
    pub fn store_success_output(&self) -> bool {
        self.store_success_output
    }

    /// Returns true if failure output should be stored.
    pub fn store_failure_output(&self) -> bool {
        self.store_failure_output
    }

    /// Returns the policy controlling which skipped tests should be emitted as
    /// `<testcase>` elements with a `<skipped>` child.
    pub fn report_skipped(&self) -> ReportSkipPolicy {
        self.report_skipped
    }

    /// Returns the flaky-fail status for JUnit reporting.
    pub fn flaky_fail_status(&self) -> JunitFlakyFailStatus {
        self.flaky_fail_status
    }

    /// Creates a `JunitConfig` directly for unit tests, bypassing the profile
    /// inheritance chain.
    #[cfg(test)]
    pub(crate) fn new_for_test(
        path: Utf8PathBuf,
        report_name: &'cfg str,
        report_skipped: ReportSkipPolicy,
    ) -> Self {
        Self {
            path,
            report_name,
            store_success_output: false,
            store_failure_output: false,
            report_skipped,
            flaky_fail_status: JunitFlakyFailStatus::Failure,
        }
    }
}

/// Pre-resolved JUnit settings from the profile inheritance chain.
#[derive(Clone, Debug)]
pub(in crate::config) struct JunitSettings<'cfg> {
    pub(in crate::config) path: Option<&'cfg Utf8Path>,
    pub(in crate::config) report_name: &'cfg str,
    pub(in crate::config) store_success_output: bool,
    pub(in crate::config) store_failure_output: bool,
    pub(in crate::config) report_skipped: ReportSkipPolicy,
    pub(in crate::config) flaky_fail_status: JunitFlakyFailStatus,
}

#[derive(Clone, Debug)]
pub(in crate::config) struct DefaultJunitImpl {
    pub(in crate::config) path: Option<Utf8PathBuf>,
    pub(in crate::config) report_name: String,
    pub(in crate::config) store_success_output: bool,
    pub(in crate::config) store_failure_output: bool,
    pub(in crate::config) report_skipped: ReportSkipPolicy,
    pub(in crate::config) flaky_fail_status: JunitFlakyFailStatus,
}

impl DefaultJunitImpl {
    // Default values have all fields defined on them.
    pub(crate) fn for_default_profile(data: JunitImpl) -> Self {
        DefaultJunitImpl {
            path: data.path,
            report_name: data
                .report_name
                .expect("junit.report present in default profile"),
            store_success_output: data
                .store_success_output
                .expect("junit.store-success-output present in default profile"),
            store_failure_output: data
                .store_failure_output
                .expect("junit.store-failure-output present in default profile"),
            report_skipped: data
                .report_skipped
                .expect("junit.report-skipped present in default profile"),
            flaky_fail_status: data
                .flaky_fail_status
                .expect("junit.flaky-fail-status present in default profile"),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(feature = "config-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "config-schema", schemars(deny_unknown_fields))]
#[serde(rename_all = "kebab-case")]
pub(in crate::config) struct JunitImpl {
    /// Path to write the JUnit XML report to. If unset, JUnit reporting is
    /// disabled.
    #[serde(default)]
    #[cfg_attr(
        feature = "config-schema",
        schemars(schema_with = "String::json_schema")
    )]
    pub(in crate::config) path: Option<Utf8PathBuf>,
    /// Name for the JUnit XML report.
    #[serde(default)]
    pub(in crate::config) report_name: Option<String>,
    /// Whether to store successful test output in the JUnit XML report.
    #[serde(default)]
    pub(in crate::config) store_success_output: Option<bool>,
    /// Whether to store failed test output in the JUnit XML report.
    #[serde(default)]
    pub(in crate::config) store_failure_output: Option<bool>,
    /// Which skipped tests to emit as `<testcase>` elements with a `<skipped>`
    /// child in the JUnit XML report.
    #[serde(default)]
    pub(in crate::config) report_skipped: Option<ReportSkipPolicy>,
    /// How flaky-fail tests are reported in the JUnit XML report.
    #[serde(default)]
    pub(in crate::config) flaky_fail_status: Option<JunitFlakyFailStatus>,
}

#[cfg(test)]
mod tests {
    use crate::config::{core::NextestConfig, elements::ReportSkipPolicy, utils::test_helpers::*};
    use camino_tempfile::tempdir;
    use indoc::indoc;
    use nextest_filtering::ParseContext;

    fn report_skipped_for(config_contents: &str, profile: &str) -> ReportSkipPolicy {
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

        nextest_config
            .profile(profile)
            .expect("profile should exist")
            .apply_build_platforms(&build_platforms())
            .junit()
            .expect("junit config should be present")
            .report_skipped()
    }

    #[test]
    fn report_skipped_defaults_to_none() {
        // When only a path is set, store-skipped must default to "none" to keep
        // machine-readable output stable.
        let config = indoc! {r#"
            [profile.default.junit]
            path = "junit.xml"
        "#};
        assert_eq!(
            report_skipped_for(config, "default"),
            ReportSkipPolicy::None
        );
    }

    #[test]
    fn report_skipped_can_be_set_to_ignored() {
        let config = indoc! {r#"
            [profile.default.junit]
            path = "junit.xml"
            report-skipped = "ignored"
        "#};
        assert_eq!(
            report_skipped_for(config, "default"),
            ReportSkipPolicy::Ignored
        );
    }

    #[test]
    fn report_skipped_can_be_set_to_all() {
        let config = indoc! {r#"
            [profile.default.junit]
            path = "junit.xml"
            report-skipped = "all"
        "#};
        assert_eq!(report_skipped_for(config, "default"), ReportSkipPolicy::All);
    }
}
