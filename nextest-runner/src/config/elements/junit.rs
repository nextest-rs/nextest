// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// Controls how flaky-fail tests are reported in JUnit XML output.
///
/// Flaky-fail tests are tests that eventually passed on retry but are configured
/// with `flaky-result = "fail"`. This setting controls whether they appear as
/// failures or successes in the JUnit report.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
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

    /// Returns the flaky-fail status for JUnit reporting.
    pub fn flaky_fail_status(&self) -> JunitFlakyFailStatus {
        self.flaky_fail_status
    }
}

/// Pre-resolved JUnit settings from the profile inheritance chain.
#[derive(Clone, Debug)]
pub(in crate::config) struct JunitSettings<'cfg> {
    pub(in crate::config) path: Option<&'cfg Utf8Path>,
    pub(in crate::config) report_name: &'cfg str,
    pub(in crate::config) store_success_output: bool,
    pub(in crate::config) store_failure_output: bool,
    pub(in crate::config) flaky_fail_status: JunitFlakyFailStatus,
}

#[derive(Clone, Debug)]
pub(in crate::config) struct DefaultJunitImpl {
    pub(in crate::config) path: Option<Utf8PathBuf>,
    pub(in crate::config) report_name: String,
    pub(in crate::config) store_success_output: bool,
    pub(in crate::config) store_failure_output: bool,
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
            flaky_fail_status: data
                .flaky_fail_status
                .expect("junit.flaky-fail-status present in default profile"),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::config) struct JunitImpl {
    #[serde(default)]
    pub(in crate::config) path: Option<Utf8PathBuf>,
    #[serde(default)]
    pub(in crate::config) report_name: Option<String>,
    #[serde(default)]
    pub(in crate::config) store_success_output: Option<bool>,
    #[serde(default)]
    pub(in crate::config) store_failure_output: Option<bool>,
    #[serde(default)]
    pub(in crate::config) flaky_fail_status: Option<JunitFlakyFailStatus>,
}
