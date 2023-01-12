// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration support for nextest.

mod identifier;
mod slow_timeout;
mod threads_required;
use crate::{
    errors::{
        provided_by_tool, ConfigParseError, ConfigParseErrorKind, ConfigParseOverrideError,
        InvalidCustomTestGroupName, ProfileNotFound, TestThreadsParseError,
        ToolConfigFileParseError, UnknownTestGroupError,
    },
    platform::BuildPlatforms,
    reporter::{FinalStatusLevel, StatusLevel, TestOutputDisplay},
};
use camino::{Utf8Path, Utf8PathBuf};
use config::{builder::DefaultState, Config, ConfigBuilder, File, FileFormat, FileSourceFile};
use guppy::graph::{cargo::BuildPlatform, PackageGraph};
pub use identifier::*;
use nextest_filtering::{FilteringExpr, TestQuery};
use once_cell::sync::Lazy;
use serde::Deserialize;
pub use slow_timeout::*;
use smol_str::SmolStr;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    str::FromStr,
    time::Duration,
};
use target_spec::TargetSpec;
pub use threads_required::*;

#[cfg(test)]
mod test_helpers;
#[cfg(test)]
use test_helpers::*;

/// Gets the number of available CPUs and caches the value.
#[inline]
pub fn get_num_cpus() -> usize {
    static NUM_CPUS: Lazy<usize> = Lazy::new(|| match std::thread::available_parallelism() {
        Ok(count) => count.into(),
        Err(err) => {
            log::warn!("unable to determine num-cpus ({err}), assuming 1 logical CPU");
            1
        }
    });

    *NUM_CPUS
}

/// Overall configuration for nextest.
///
/// This is the root data structure for nextest configuration. Most runner-specific configuration is managed
/// through [profiles](NextestProfile), obtained through the [`profile`](Self::profile) method.
///
/// For more about configuration, see
/// [Configuration](https://nexte.st/book/configuration.html) in the nextest
/// book.
#[derive(Clone, Debug)]
pub struct NextestConfig {
    workspace_root: Utf8PathBuf,
    inner: NextestConfigImpl,
    overrides: NextestOverridesImpl,
}

impl NextestConfig {
    /// The default location of the config within the path: `.config/nextest.toml`, used to read the
    /// config from the given directory.
    pub const CONFIG_PATH: &'static str = ".config/nextest.toml";

    /// Contains the default config as a TOML file.
    ///
    /// Repository-specific configuration is layered on top of the default config.
    pub const DEFAULT_CONFIG: &'static str = include_str!("../default-config.toml");

    /// Environment configuration uses this prefix, plus a _.
    pub const ENVIRONMENT_PREFIX: &'static str = "NEXTEST";

    /// The name of the default profile.
    pub const DEFAULT_PROFILE: &'static str = "default";

    /// The name of the default profile used for miri.
    pub const DEFAULT_MIRI_PROFILE: &'static str = "default-miri";

    /// A list containing the names of the Nextest defined reserved profile names.
    pub const DEFAULT_PROFILES: &'static [&'static str] =
        &[Self::DEFAULT_PROFILE, Self::DEFAULT_MIRI_PROFILE];

    /// Reads the nextest config from the given file, or if not specified from `.config/nextest.toml`
    /// in the workspace root.
    ///
    /// `tool_config_files` are lower priority than `config_file` but higher priority than the
    /// default config. Files in `tool_config_files` that come earlier are higher priority than those
    /// that come later.
    ///
    /// If no config files are specified and this file doesn't have `.config/nextest.toml`, uses the
    /// default config options.
    pub fn from_sources<'a, I>(
        workspace_root: impl Into<Utf8PathBuf>,
        graph: &PackageGraph,
        config_file: Option<&Utf8Path>,
        tool_config_files: impl IntoIterator<IntoIter = I>,
    ) -> Result<Self, ConfigParseError>
    where
        I: Iterator<Item = &'a ToolConfigFile> + DoubleEndedIterator,
    {
        Self::from_sources_impl(
            workspace_root,
            graph,
            config_file,
            tool_config_files.into_iter(),
            |config_file, tool, unknown| {
                let mut unknown_str = String::new();
                if unknown.len() == 1 {
                    // Print this on the same line.
                    unknown_str.push(' ');
                    unknown_str.push_str(unknown.iter().next().unwrap());
                } else {
                    for ignored_key in unknown {
                        unknown_str.push('\n');
                        unknown_str.push_str("  - ");
                        unknown_str.push_str(ignored_key);
                    }
                }

                log::warn!(
                    "ignoring unknown configuration keys in config file {config_file}{}:{unknown_str}",
                    provided_by_tool(tool),
                )
            },
        )
    }

    // A custom unknown_callback can be passed in while testing.
    fn from_sources_impl<'a, I>(
        workspace_root: impl Into<Utf8PathBuf>,
        graph: &PackageGraph,
        config_file: Option<&Utf8Path>,
        tool_config_files: impl IntoIterator<IntoIter = I>,
        mut unknown_callback: impl FnMut(&Utf8Path, Option<&str>, &BTreeSet<String>),
    ) -> Result<Self, ConfigParseError>
    where
        I: Iterator<Item = &'a ToolConfigFile> + DoubleEndedIterator,
    {
        let workspace_root = workspace_root.into();
        let tool_config_files_rev = tool_config_files.into_iter().rev();
        let (inner, overrides) = Self::read_from_sources(
            graph,
            &workspace_root,
            config_file,
            tool_config_files_rev,
            &mut unknown_callback,
        )?;
        Ok(Self {
            workspace_root,
            inner,
            overrides,
        })
    }

    /// Returns the default nextest config.
    #[cfg(test)]
    pub(crate) fn default_config(workspace_root: impl Into<Utf8PathBuf>) -> Self {
        use itertools::Itertools;

        let config = Self::make_default_config()
            .build()
            .expect("default config is always valid");

        let mut unknown = BTreeSet::new();
        let deserialized: NextestConfigDeserialize =
            serde_ignored::deserialize(config, |path: serde_ignored::Path| {
                unknown.insert(path.to_string());
            })
            .expect("default config is always valid");

        // Make sure there aren't any unknown keys in the default config, since it is
        // embedded/shipped with this binary.
        if !unknown.is_empty() {
            panic!(
                "found unknown keys in default config: {}",
                unknown.iter().join(", ")
            );
        }

        Self {
            workspace_root: workspace_root.into(),
            inner: deserialized.into_config_impl(),
            // The default config does not (cannot) have overrides.
            overrides: NextestOverridesImpl::default(),
        }
    }

    /// Returns the profile with the given name, or an error if a profile was specified but not
    /// found.
    pub fn profile(
        &self,
        name: impl AsRef<str>,
    ) -> Result<NextestProfile<'_, PreBuildPlatform>, ProfileNotFound> {
        self.make_profile(name.as_ref())
    }

    // ---
    // Helper methods
    // ---

    fn read_from_sources<'a>(
        graph: &PackageGraph,
        workspace_root: &Utf8Path,
        file: Option<&Utf8Path>,
        tool_config_files_rev: impl Iterator<Item = &'a ToolConfigFile>,
        unknown_callback: &mut impl FnMut(&Utf8Path, Option<&str>, &BTreeSet<String>),
    ) -> Result<(NextestConfigImpl, NextestOverridesImpl), ConfigParseError> {
        // First, get the default config.
        let mut composite_builder = Self::make_default_config();

        // Overrides are handled additively.
        // Note that they're stored in reverse order here, and are flipped over at the end.
        let mut overrides_impl = NextestOverridesImpl::default();

        let mut known_groups = BTreeSet::new();

        // Next, merge in tool configs.
        for ToolConfigFile { config_file, tool } in tool_config_files_rev {
            let source = File::new(config_file.as_str(), FileFormat::Toml);
            Self::deserialize_individual_config(
                graph,
                workspace_root,
                config_file,
                Some(tool),
                source.clone(),
                &mut overrides_impl,
                unknown_callback,
                &mut known_groups,
            )?;

            // This is the final, composite builder used at the end.
            composite_builder = composite_builder.add_source(source);
        }

        // Next, merge in the config from the given file.
        let (config_file, source) = match file {
            Some(file) => (file.to_owned(), File::new(file.as_str(), FileFormat::Toml)),
            None => {
                let config_file = workspace_root.join(Self::CONFIG_PATH);
                let source = File::new(config_file.as_str(), FileFormat::Toml).required(false);
                (config_file, source)
            }
        };

        Self::deserialize_individual_config(
            graph,
            workspace_root,
            &config_file,
            None,
            source.clone(),
            &mut overrides_impl,
            unknown_callback,
            &mut known_groups,
        )?;

        composite_builder = composite_builder.add_source(source);

        // The unknown set is ignored here because any values in it have already been reported in
        // deserialize_individual_config.
        let (config, _unknown) = Self::build_and_deserialize_config(&composite_builder)
            .map_err(|kind| ConfigParseError::new(config_file, None, kind))?;

        // Reverse all the overrides at the end.
        overrides_impl.default.reverse();
        for override_ in overrides_impl.other.values_mut() {
            override_.reverse();
        }

        Ok((config.into_config_impl(), overrides_impl))
    }

    #[allow(clippy::too_many_arguments)]
    fn deserialize_individual_config(
        graph: &PackageGraph,
        workspace_root: &Utf8Path,
        config_file: &Utf8Path,
        tool: Option<&str>,
        source: File<FileSourceFile, FileFormat>,
        overrides_impl: &mut NextestOverridesImpl,
        unknown_callback: &mut impl FnMut(&Utf8Path, Option<&str>, &BTreeSet<String>),
        known_groups: &mut BTreeSet<CustomTestGroup>,
    ) -> Result<(), ConfigParseError> {
        // Try building default builder + this file to get good error attribution and handle
        // overrides additively.
        let default_builder = Self::make_default_config();
        let this_builder = default_builder.add_source(source);
        let (this_config, unknown) = Self::build_and_deserialize_config(&this_builder)
            .map_err(|kind| ConfigParseError::new(config_file, tool, kind))?;

        if !unknown.is_empty() {
            unknown_callback(config_file, tool, &unknown);
        }

        // Check that test groups are named as expected.
        let (valid_groups, invalid_groups): (BTreeSet<_>, _) =
            this_config.test_groups.keys().cloned().partition(|group| {
                if let Some(tool) = tool {
                    // The first component must be the tool name.
                    group
                        .as_identifier()
                        .tool_components()
                        .map_or(false, |(tool_name, _)| tool_name == tool)
                } else {
                    // If a tool is not specified, it must *not* be a tool identifier.
                    !group.as_identifier().is_tool_identifier()
                }
            });

        if !invalid_groups.is_empty() {
            let kind = if tool.is_some() {
                ConfigParseErrorKind::InvalidTestGroupsDefinedByTool(invalid_groups)
            } else {
                ConfigParseErrorKind::InvalidTestGroupsDefined(invalid_groups)
            };
            return Err(ConfigParseError::new(config_file, tool, kind));
        }

        known_groups.extend(valid_groups);

        let this_config = this_config.into_config_impl();

        let unknown_default_profiles: Vec<_> = this_config
            .all_profiles()
            .filter(|p| p.starts_with("default-") && !NextestConfig::DEFAULT_PROFILES.contains(p))
            .collect();
        if !unknown_default_profiles.is_empty() {
            log::warn!(
                "unknown profiles in the reserved `default-` namespace in config file {}{}:",
                config_file
                    .strip_prefix(workspace_root)
                    .unwrap_or(config_file),
                provided_by_tool(tool),
            );

            for profile in unknown_default_profiles {
                log::warn!("  {profile}");
            }
        }

        // Compile the overrides for this file.
        let this_overrides = NextestOverridesImpl::new(graph, &this_config)
            .map_err(|kind| ConfigParseError::new(config_file, tool, kind))?;

        // Check that all overrides specify known test groups.
        let mut unknown_group_errors = Vec::new();
        let mut check_test_group = |profile_name: &str, test_group: Option<&TestGroup>| {
            if let Some(TestGroup::Custom(group)) = test_group {
                if !known_groups.contains(group) {
                    unknown_group_errors.push(UnknownTestGroupError {
                        profile_name: profile_name.to_owned(),
                        name: TestGroup::Custom(group.clone()),
                    });
                }
            }
        };

        this_overrides.default.iter().for_each(|override_| {
            check_test_group("default", override_.data.test_group.as_ref());
        });

        // Check that override test groups are known.
        this_overrides
            .other
            .iter()
            .for_each(|(profile_name, overrides)| {
                overrides.iter().for_each(|override_| {
                    check_test_group(profile_name, override_.data.test_group.as_ref());
                });
            });

        // If there were any unknown groups, error out.
        if !unknown_group_errors.is_empty() {
            let known_groups = TestGroup::make_all_groups(known_groups.iter().cloned()).collect();
            return Err(ConfigParseError::new(
                config_file,
                tool,
                ConfigParseErrorKind::UnknownTestGroups {
                    errors: unknown_group_errors,
                    known_groups,
                },
            ));
        }

        // Grab the overrides for this config. Add them in reversed order (we'll flip it around at the end).
        overrides_impl
            .default
            .extend(this_overrides.default.into_iter().rev());
        for (name, overrides) in this_overrides.other {
            overrides_impl
                .other
                .entry(name)
                .or_default()
                .extend(overrides.into_iter().rev());
        }

        Ok(())
    }

    fn make_default_config() -> ConfigBuilder<DefaultState> {
        Config::builder().add_source(File::from_str(Self::DEFAULT_CONFIG, FileFormat::Toml))
    }

    fn make_profile(
        &self,
        name: &str,
    ) -> Result<NextestProfile<'_, PreBuildPlatform>, ProfileNotFound> {
        let custom_profile = self.inner.get_profile(name)?;

        // The profile was found: construct the NextestProfile.
        let mut store_dir = self.workspace_root.join(&self.inner.store.dir);
        store_dir.push(name);

        // Grab the overrides as well.
        let overrides = self
            .overrides
            .other
            .get(name)
            .into_iter()
            .flatten()
            .chain(self.overrides.default.iter())
            .cloned()
            .collect();

        Ok(NextestProfile {
            store_dir,
            default_profile: &self.inner.default_profile,
            custom_profile,
            test_groups: &self.inner.test_groups,
            overrides,
        })
    }

    /// This returns a tuple of (config, ignored paths).
    fn build_and_deserialize_config(
        builder: &ConfigBuilder<DefaultState>,
    ) -> Result<(NextestConfigDeserialize, BTreeSet<String>), ConfigParseErrorKind> {
        let config = builder
            .build_cloned()
            .map_err(|error| ConfigParseErrorKind::BuildError(Box::new(error)))?;

        let mut ignored = BTreeSet::new();
        let mut cb = |path: serde_ignored::Path| {
            ignored.insert(path.to_string());
        };
        let ignored_de = serde_ignored::Deserializer::new(config, &mut cb);
        let config: NextestConfigDeserialize = serde_path_to_error::deserialize(ignored_de)
            .map_err(|error| ConfigParseErrorKind::DeserializeError(Box::new(error)))?;

        Ok((config, ignored))
    }
}

/// A tool-specific config file.
///
/// Tool-specific config files are lower priority than repository configs, but higher priority than
/// the default config shipped with nextest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolConfigFile {
    /// The name of the tool.
    pub tool: String,

    /// The path to the config file.
    pub config_file: Utf8PathBuf,
}

impl FromStr for ToolConfigFile {
    type Err = ToolConfigFileParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.split_once(':') {
            Some((tool, config_file)) => {
                if tool.is_empty() {
                    Err(ToolConfigFileParseError::EmptyToolName {
                        input: input.to_owned(),
                    })
                } else if config_file.is_empty() {
                    Err(ToolConfigFileParseError::EmptyConfigFile {
                        input: input.to_owned(),
                    })
                } else {
                    let config_file = Utf8Path::new(config_file);
                    if config_file.is_absolute() {
                        Ok(Self {
                            tool: tool.to_owned(),
                            config_file: Utf8PathBuf::from(config_file),
                        })
                    } else {
                        Err(ToolConfigFileParseError::ConfigFileNotAbsolute {
                            config_file: config_file.to_owned(),
                        })
                    }
                }
            }
            None => Err(ToolConfigFileParseError::InvalidFormat {
                input: input.to_owned(),
            }),
        }
    }
}

/// The state of nextest profiles before build platforms have been applied.
#[derive(Clone, Debug)]
pub struct PreBuildPlatform {}

/// The state of nextest profiles after build platforms have been applied.
#[derive(Clone, Debug)]
pub struct FinalConfig {
    host_eval: bool,
    target_eval: bool,
}

/// A configuration profile for nextest. Contains most configuration used by the nextest runner.
///
/// Returned by [`NextestConfig::profile`].
#[derive(Clone, Debug)]
pub struct NextestProfile<'cfg, State = FinalConfig> {
    store_dir: Utf8PathBuf,
    default_profile: &'cfg DefaultProfileImpl,
    custom_profile: Option<&'cfg CustomProfileImpl>,
    test_groups: &'cfg BTreeMap<CustomTestGroup, TestGroupConfig>,
    overrides: Vec<ProfileOverrideImpl<State>>,
}

impl<'cfg, State> NextestProfile<'cfg, State> {
    /// Returns the absolute profile-specific store directory.
    pub fn store_dir(&self) -> &Utf8Path {
        &self.store_dir
    }

    /// Returns the test group configuration for this profile.
    pub fn test_group_config(&self) -> &'cfg BTreeMap<CustomTestGroup, TestGroupConfig> {
        self.test_groups
    }
}

impl<'cfg> NextestProfile<'cfg, PreBuildPlatform> {
    /// Applies build platforms to make the profile ready for evaluation.
    ///
    /// This is a separate step from parsing the config and reading a profile so that cargo-nextest
    /// can tell users about configuration parsing errors before building the binary list.
    pub fn apply_build_platforms(self, build_platforms: &BuildPlatforms) -> NextestProfile<'cfg> {
        let overrides = self
            .overrides
            .into_iter()
            .map(|override_| override_.apply_build_platforms(build_platforms))
            .collect();
        NextestProfile {
            store_dir: self.store_dir,
            default_profile: self.default_profile,
            custom_profile: self.custom_profile,
            test_groups: self.test_groups,
            overrides,
        }
    }
}

impl<'cfg> NextestProfile<'cfg, FinalConfig> {
    /// Returns the retry count for this profile.
    pub fn retries(&self) -> RetryPolicy {
        self.custom_profile
            .and_then(|profile| profile.retries)
            .unwrap_or(self.default_profile.retries)
    }

    /// Returns the number of threads to run against for this profile.
    pub fn test_threads(&self) -> TestThreads {
        self.custom_profile
            .and_then(|profile| profile.test_threads)
            .unwrap_or(self.default_profile.test_threads)
    }

    /// Returns the number of threads required for each test.
    pub fn threads_required(&self) -> ThreadsRequired {
        self.custom_profile
            .and_then(|profile| profile.threads_required)
            .unwrap_or(self.default_profile.threads_required)
    }

    /// Returns the time after which tests are treated as slow for this profile.
    pub fn slow_timeout(&self) -> SlowTimeout {
        self.custom_profile
            .and_then(|profile| profile.slow_timeout)
            .unwrap_or(self.default_profile.slow_timeout)
    }

    /// Returns the time after which a child process that hasn't closed its handles is marked as
    /// leaky.
    pub fn leak_timeout(&self) -> Duration {
        self.custom_profile
            .and_then(|profile| profile.leak_timeout)
            .unwrap_or(self.default_profile.leak_timeout)
    }

    /// Returns the test status level.
    pub fn status_level(&self) -> StatusLevel {
        self.custom_profile
            .and_then(|profile| profile.status_level)
            .unwrap_or(self.default_profile.status_level)
    }

    /// Returns the test status level at the end of the run.
    pub fn final_status_level(&self) -> FinalStatusLevel {
        self.custom_profile
            .and_then(|profile| profile.final_status_level)
            .unwrap_or(self.default_profile.final_status_level)
    }

    /// Returns the failure output config for this profile.
    pub fn failure_output(&self) -> TestOutputDisplay {
        self.custom_profile
            .and_then(|profile| profile.failure_output)
            .unwrap_or(self.default_profile.failure_output)
    }

    /// Returns the failure output config for this profile.
    pub fn success_output(&self) -> TestOutputDisplay {
        self.custom_profile
            .and_then(|profile| profile.success_output)
            .unwrap_or(self.default_profile.success_output)
    }

    /// Returns the fail-fast config for this profile.
    pub fn fail_fast(&self) -> bool {
        self.custom_profile
            .and_then(|profile| profile.fail_fast)
            .unwrap_or(self.default_profile.fail_fast)
    }

    /// Returns override settings for individual tests.
    pub fn overrides_for(&self, query: &TestQuery<'_>) -> ProfileOverrides {
        self.overrides_for_impl(query)
    }

    /// Returns override settings for individual tests, with sources attached.
    pub(crate) fn overrides_with_source_for(
        &self,
        query: &TestQuery<'_>,
    ) -> ProfileOverrides<&ProfileOverrideImpl<FinalConfig>> {
        self.overrides_for_impl(query)
    }

    fn overrides_for_impl<'p, Source: OverrideSource<'p>>(
        &'p self,
        query: &TestQuery<'_>,
    ) -> ProfileOverrides<Source> {
        let mut threads_required = None;
        let mut retries = None;
        let mut slow_timeout = None;
        let mut leak_timeout = None;
        let mut test_group = None;

        for override_ in &self.overrides {
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

    /// Returns the JUnit configuration for this profile.
    pub fn junit(&self) -> Option<NextestJunitConfig<'cfg>> {
        let path = self
            .custom_profile
            .map(|profile| &profile.junit.path)
            .unwrap_or(&self.default_profile.junit.path)
            .as_deref();

        path.map(|path| {
            let path = self.store_dir.join(path);
            let report_name = self
                .custom_profile
                .and_then(|profile| profile.junit.report_name.as_deref())
                .unwrap_or(&self.default_profile.junit.report_name);
            NextestJunitConfig { path, report_name }
        })
    }
}

/// Override settings for individual tests.
///
/// Returned by [`NextestProfile::overrides_for`].
///
/// The `Source` parameter tracks an optional source.
#[derive(Clone, Debug)]
pub struct ProfileOverrides<Source = ()> {
    threads_required: Option<(ThreadsRequired, Source)>,
    retries: Option<(RetryPolicy, Source)>,
    slow_timeout: Option<(SlowTimeout, Source)>,
    leak_timeout: Option<(Duration, Source)>,
    test_group: Option<(TestGroup, Source)>,
}

trait OverrideSource<'p>: Sized {
    fn track_source<T>(
        value: Option<T>,
        source: &'p ProfileOverrideImpl<FinalConfig>,
    ) -> Option<(T, Self)>;
}

impl<'p> OverrideSource<'p> for () {
    fn track_source<T>(
        value: Option<T>,
        _source: &'p ProfileOverrideImpl<FinalConfig>,
    ) -> Option<(T, Self)> {
        value.map(|value| (value, ()))
    }
}

impl<'p> OverrideSource<'p> for &'p ProfileOverrideImpl<FinalConfig> {
    fn track_source<T>(
        value: Option<T>,
        source: &'p ProfileOverrideImpl<FinalConfig>,
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

/// JUnit configuration for nextest, returned by a [`NextestProfile`].
#[derive(Clone, Debug)]
pub struct NextestJunitConfig<'cfg> {
    path: Utf8PathBuf,
    report_name: &'cfg str,
}

impl<'cfg> NextestJunitConfig<'cfg> {
    /// Returns the absolute path to the JUnit report.
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// Returns the name of the JUnit report.
    pub fn report_name(&self) -> &'cfg str {
        self.report_name
    }
}

#[derive(Clone, Debug)]
struct NextestConfigImpl {
    store: StoreConfigImpl,
    test_groups: BTreeMap<CustomTestGroup, TestGroupConfig>,
    default_profile: DefaultProfileImpl,
    other_profiles: HashMap<String, CustomProfileImpl>,
}

impl NextestConfigImpl {
    fn get_profile(&self, profile: &str) -> Result<Option<&CustomProfileImpl>, ProfileNotFound> {
        let custom_profile = match profile {
            NextestConfig::DEFAULT_PROFILE => None,
            other => Some(
                self.other_profiles
                    .get(other)
                    .ok_or_else(|| ProfileNotFound::new(profile, self.all_profiles()))?,
            ),
        };
        Ok(custom_profile)
    }

    fn all_profiles(&self) -> impl Iterator<Item = &str> {
        self.other_profiles
            .keys()
            .map(|key| key.as_str())
            .chain(std::iter::once(NextestConfig::DEFAULT_PROFILE))
    }
}

// This is the form of `NextestConfig` that gets deserialized.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct NextestConfigDeserialize {
    store: StoreConfigImpl,
    #[serde(default)]
    test_groups: BTreeMap<CustomTestGroup, TestGroupConfig>,
    #[serde(rename = "profile")]
    profiles: HashMap<String, CustomProfileImpl>,
}

impl NextestConfigDeserialize {
    fn into_config_impl(mut self) -> NextestConfigImpl {
        let p = self
            .profiles
            .remove("default")
            .expect("default profile should exist");
        let default_profile = DefaultProfileImpl::new(p);

        NextestConfigImpl {
            store: self.store,
            default_profile,
            test_groups: self.test_groups,
            other_profiles: self.profiles,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct StoreConfigImpl {
    dir: Utf8PathBuf,
}

#[derive(Clone, Debug)]
struct DefaultProfileImpl {
    test_threads: TestThreads,
    threads_required: ThreadsRequired,
    retries: RetryPolicy,
    status_level: StatusLevel,
    final_status_level: FinalStatusLevel,
    failure_output: TestOutputDisplay,
    success_output: TestOutputDisplay,
    fail_fast: bool,
    slow_timeout: SlowTimeout,
    leak_timeout: Duration,
    overrides: Vec<ProfileOverrideSource>,
    junit: DefaultJunitImpl,
}

impl DefaultProfileImpl {
    fn new(p: CustomProfileImpl) -> Self {
        Self {
            test_threads: p
                .test_threads
                .expect("test-threads present in default profile"),
            threads_required: p
                .threads_required
                .expect("threads-required present in default profile"),
            retries: p.retries.expect("retries present in default profile"),
            status_level: p
                .status_level
                .expect("status-level present in default profile"),
            final_status_level: p
                .final_status_level
                .expect("final-status-level present in default profile"),
            failure_output: p
                .failure_output
                .expect("failure-output present in default profile"),
            success_output: p
                .success_output
                .expect("success-output present in default profile"),
            fail_fast: p.fail_fast.expect("fail-fast present in default profile"),
            slow_timeout: p
                .slow_timeout
                .expect("slow-timeout present in default profile"),
            leak_timeout: p
                .leak_timeout
                .expect("leak-timeout present in default profile"),
            overrides: p.overrides,
            junit: DefaultJunitImpl {
                path: p.junit.path,
                report_name: p
                    .junit
                    .report_name
                    .expect("junit.report present in default profile"),
            },
        }
    }
}

/// Type for the test-threads config key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestThreads {
    /// Run tests with a specified number of threads.
    Count(usize),

    /// Run tests with a number of threads equal to the logical CPU count.
    NumCpus,
}

impl TestThreads {
    /// Gets the actual number of test threads computed at runtime.
    pub fn compute(self) -> usize {
        match self {
            Self::Count(threads) => threads,
            Self::NumCpus => get_num_cpus(),
        }
    }
}

impl FromStr for TestThreads {
    type Err = TestThreadsParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "num-cpus" {
            return Ok(Self::NumCpus);
        }

        match s.parse::<isize>() {
            Err(e) => Err(TestThreadsParseError::new(format!(
                "Error: {e} parsing {s}"
            ))),
            Ok(0) => Err(TestThreadsParseError::new("jobs may not be 0")),
            Ok(j) if j < 0 => Ok(TestThreads::Count(
                (get_num_cpus() as isize + j).max(1) as usize
            )),
            Ok(j) => Ok(TestThreads::Count(j as usize)),
        }
    }
}

impl fmt::Display for TestThreads {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Count(threads) => write!(f, "{}", threads),
            Self::NumCpus => write!(f, "num-cpus"),
        }
    }
}

impl<'de> Deserialize<'de> for TestThreads {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;

        impl<'de2> serde::de::Visitor<'de2> for V {
            type Value = TestThreads;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "an integer or the string \"num-cpus\"")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v == "num-cpus" {
                    Ok(TestThreads::NumCpus)
                } else {
                    Err(serde::de::Error::invalid_value(
                        serde::de::Unexpected::Str(v),
                        &self,
                    ))
                }
            }

            // Note that TOML uses i64, not u64.
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v.cmp(&0) {
                    Ordering::Greater => Ok(TestThreads::Count(v as usize)),
                    Ordering::Less => Ok(TestThreads::Count(
                        (get_num_cpus() as i64 + v).max(1) as usize
                    )),
                    Ordering::Equal => Err(serde::de::Error::invalid_value(
                        serde::de::Unexpected::Signed(v),
                        &self,
                    )),
                }
            }
        }

        deserializer.deserialize_any(V)
    }
}

/// Type for the retry config key.
#[derive(Debug, Copy, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "backoff", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RetryPolicy {
    /// Fixed backoff.
    #[serde(rename_all = "kebab-case")]
    Fixed {
        /// Maximum retry count.
        count: usize,

        /// Delay between retries.
        #[serde(default, with = "humantime_serde")]
        delay: Duration,

        /// If set to true, randomness will be added to the delay on each retry attempt.
        #[serde(default)]
        jitter: bool,
    },

    /// Exponential backoff.
    #[serde(rename_all = "kebab-case")]
    Exponential {
        /// Maximum retry count.
        count: usize,

        /// Delay between retries. Not optional for exponential backoff.
        #[serde(with = "humantime_serde")]
        delay: Duration,

        /// If set to true, randomness will be added to the delay on each retry attempt.
        #[serde(default)]
        jitter: bool,

        /// If set, limits the delay between retries.
        #[serde(default, with = "humantime_serde")]
        max_delay: Option<Duration>,
    },
}

impl Default for RetryPolicy {
    #[inline]
    fn default() -> Self {
        Self::new_without_delay(0)
    }
}

impl RetryPolicy {
    /// Create new policy with no delay between retries.
    pub fn new_without_delay(count: usize) -> Self {
        Self::Fixed {
            count,
            delay: Duration::ZERO,
            jitter: false,
        }
    }

    /// Returns the number of retries.
    pub fn count(&self) -> usize {
        match self {
            Self::Fixed { count, .. } | Self::Exponential { count, .. } => *count,
        }
    }
}

fn deserialize_retry_policy<'de, D>(deserializer: D) -> Result<Option<RetryPolicy>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;

    impl<'de2> serde::de::Visitor<'de2> for V {
        type Value = Option<RetryPolicy>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(
                formatter,
                "a table ({{ count = 5, backoff = \"exponential\", delay = \"1s\", max-delay = \"10s\", jitter = true }}) or a number (5)"
            )
        }

        // Note that TOML uses i64, not u64.
        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            match v.cmp(&0) {
                Ordering::Greater | Ordering::Equal => {
                    Ok(Some(RetryPolicy::new_without_delay(v as usize)))
                }
                Ordering::Less => Err(serde::de::Error::invalid_value(
                    serde::de::Unexpected::Signed(v),
                    &self,
                )),
            }
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de2>,
        {
            RetryPolicy::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Some)
        }
    }

    // Post-deserialize validation of retry policy.
    let retry_policy = deserializer.deserialize_any(V)?;
    match &retry_policy {
        Some(RetryPolicy::Fixed {
            count: _,
            delay,
            jitter,
        }) => {
            // Jitter can't be specified if delay is 0.
            if delay.is_zero() && *jitter {
                return Err(serde::de::Error::custom(
                    "`jitter` cannot be true if `delay` isn't specified or is zero",
                ));
            }
        }
        Some(RetryPolicy::Exponential {
            count,
            delay,
            jitter: _,
            max_delay,
        }) => {
            // Count can't be zero.
            if *count == 0 {
                return Err(serde::de::Error::custom(
                    "`count` cannot be zero with exponential backoff",
                ));
            }
            // Delay can't be zero.
            if delay.is_zero() {
                return Err(serde::de::Error::custom(
                    "`delay` cannot be zero with exponential backoff",
                ));
            }
            // Max delay, if specified, can't be zero.
            if max_delay.map_or(false, |f| f.is_zero()) {
                return Err(serde::de::Error::custom(
                    "`max-delay` cannot be zero with exponential backoff",
                ));
            }
            // Max delay can't be less than delay.
            if max_delay.map_or(false, |max_delay| max_delay < *delay) {
                return Err(serde::de::Error::custom(
                    "`max-delay` cannot be less than delay with exponential backoff",
                ));
            }
        }
        None => {}
    }

    Ok(retry_policy)
}

/// Represents the test group a test is in.
#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum TestGroup {
    /// This test is in the named custom group.
    Custom(CustomTestGroup),

    /// This test is not in a group.
    Global,
}

impl TestGroup {
    pub(crate) fn make_all_groups(
        custom_groups: impl IntoIterator<Item = CustomTestGroup>,
    ) -> impl Iterator<Item = Self> {
        custom_groups
            .into_iter()
            .map(TestGroup::Custom)
            .chain(std::iter::once(TestGroup::Global))
    }
}

impl<'de> Deserialize<'de> for TestGroup {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Try and deserialize the group as a string. (Note: we don't deserialize a
        // `CustomTestGroup` directly because that errors out on None.
        let group = SmolStr::deserialize(deserializer)?;
        if group == "@global" {
            Ok(TestGroup::Global)
        } else {
            Ok(TestGroup::Custom(
                CustomTestGroup::new(group).map_err(serde::de::Error::custom)?,
            ))
        }
    }
}

impl FromStr for TestGroup {
    type Err = InvalidCustomTestGroupName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "@global" {
            Ok(TestGroup::Global)
        } else {
            Ok(TestGroup::Custom(CustomTestGroup::new(s.into())?))
        }
    }
}

impl fmt::Display for TestGroup {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TestGroup::Global => write!(f, "@global"),
            TestGroup::Custom(group) => write!(f, "{}", group.as_str()),
        }
    }
}

/// Represents a custom test group.
#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct CustomTestGroup(ConfigIdentifier);

impl CustomTestGroup {
    /// Creates a new custom test group, returning an error if it is invalid.
    pub fn new(name: SmolStr) -> Result<Self, InvalidCustomTestGroupName> {
        let identifier = ConfigIdentifier::new(name).map_err(InvalidCustomTestGroupName)?;
        Ok(Self(identifier))
    }

    /// Creates a new custom test group from an identifier.
    pub fn from_identifier(identifier: ConfigIdentifier) -> Self {
        Self(identifier)
    }

    /// Returns the test group as a [`ConfigIdentifier`].
    pub fn as_identifier(&self) -> &ConfigIdentifier {
        &self.0
    }

    /// Returns the test group as a string.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for CustomTestGroup {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Try and deserialize as a string.
        let identifier = SmolStr::deserialize(deserializer)?;
        Self::new(identifier).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for CustomTestGroup {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Configuration for a test group.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestGroupConfig {
    /// The maximum number of threads allowed for this test group.
    pub max_threads: TestThreads,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DefaultJunitImpl {
    #[serde(default)]
    path: Option<Utf8PathBuf>,
    report_name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CustomProfileImpl {
    #[serde(default, deserialize_with = "deserialize_retry_policy")]
    retries: Option<RetryPolicy>,
    #[serde(default)]
    test_threads: Option<TestThreads>,
    #[serde(default)]
    threads_required: Option<ThreadsRequired>,
    #[serde(default)]
    status_level: Option<StatusLevel>,
    #[serde(default)]
    final_status_level: Option<FinalStatusLevel>,
    #[serde(default)]
    failure_output: Option<TestOutputDisplay>,
    #[serde(default)]
    success_output: Option<TestOutputDisplay>,
    #[serde(default)]
    fail_fast: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_slow_timeout")]
    slow_timeout: Option<SlowTimeout>,
    #[serde(default, with = "humantime_serde::option")]
    leak_timeout: Option<Duration>,
    #[serde(default)]
    overrides: Vec<ProfileOverrideSource>,
    #[serde(default)]
    junit: JunitImpl,
}

/// Pre-compiled form of profile overrides.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ProfileOverrideSource {
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
    #[serde(default, deserialize_with = "deserialize_retry_policy")]
    retries: Option<RetryPolicy>,
    #[serde(default, deserialize_with = "deserialize_slow_timeout")]
    slow_timeout: Option<SlowTimeout>,
    #[serde(default, with = "humantime_serde::option")]
    leak_timeout: Option<Duration>,
    #[serde(default)]
    test_group: Option<TestGroup>,
}

#[derive(Clone, Debug, Default)]
struct NextestOverridesImpl {
    default: Vec<ProfileOverrideImpl<PreBuildPlatform>>,
    other: HashMap<String, Vec<ProfileOverrideImpl<PreBuildPlatform>>>,
}

impl NextestOverridesImpl {
    fn new(graph: &PackageGraph, config: &NextestConfigImpl) -> Result<Self, ConfigParseErrorKind> {
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
    ) -> Vec<ProfileOverrideImpl<PreBuildPlatform>> {
        overrides
            .iter()
            .enumerate()
            .filter_map(|(index, source)| {
                ProfileOverrideImpl::new(graph, profile_name, index, source, errors)
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProfileOverrideImpl<State> {
    id: OverrideId,
    state: State,
    data: ProfileOverrideData,
}

impl<State> ProfileOverrideImpl<State> {
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
struct ProfileOverrideData {
    target_spec: Option<TargetSpec>,
    expr: Option<(String, FilteringExpr)>,
    threads_required: Option<ThreadsRequired>,
    retries: Option<RetryPolicy>,
    slow_timeout: Option<SlowTimeout>,
    leak_timeout: Option<Duration>,
    test_group: Option<TestGroup>,
}

impl ProfileOverrideImpl<PreBuildPlatform> {
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

    fn apply_build_platforms(
        self,
        build_platforms: &BuildPlatforms,
    ) -> ProfileOverrideImpl<FinalConfig> {
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
        ProfileOverrideImpl {
            id: self.id,
            state: FinalConfig {
                host_eval,
                target_eval,
            },
            data: self.data,
        }
    }
}

impl ProfileOverrideImpl<FinalConfig> {
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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct JunitImpl {
    #[serde(default)]
    path: Option<Utf8PathBuf>,
    report_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::ConfigError;
    use guppy::graph::cargo::BuildPlatform;
    use indoc::indoc;
    use maplit::btreeset;
    use nextest_filtering::BinaryQuery;
    use std::num::NonZeroUsize;
    use tempfile::tempdir;
    use test_case::test_case;

    #[test]
    fn default_config_is_valid() {
        let default_config = NextestConfig::default_config("foo");
        default_config
            .profile(NextestConfig::DEFAULT_PROFILE)
            .expect("default profile should exist");
    }

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

    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 2

            [profile.ci]
        "#},
        BuildPlatform::Target,
        Some(RetryPolicy::new_without_delay(2))

        ; "my_test matches exactly"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "!test(=my_test)"
            retries = 2

            [profile.ci]
        "#},
        BuildPlatform::Target,
        None

        ; "not match"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(=my_test)"

            [profile.ci]
        "#},
        BuildPlatform::Target,
        None

        ; "no retries specified"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Target,
        Some(RetryPolicy::new_without_delay(2))

        ; "earlier configs override later ones"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(test)"
            retries = 2

            [profile.ci]

            [[profile.ci.overrides]]
            filter = "test(=my_test)"
            retries = 3
        "#},
        BuildPlatform::Target,
        Some(RetryPolicy::new_without_delay(3))

        ; "profile-specific configs override default ones"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "(!package(test-package)) and test(test)"
            retries = 2

            [profile.ci]

            [[profile.ci.overrides]]
            filter = "!test(=my_test_2)"
            retries = 3
        "#},
        BuildPlatform::Target,
        Some(RetryPolicy::new_without_delay(3))

        ; "no overrides match my_test exactly"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = "x86_64-unknown-linux-gnu"
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Host,
        Some(RetryPolicy::new_without_delay(2))

        ; "earlier config applied because it matches host triple"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = "aarch64-apple-darwin"
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Host,
        Some(RetryPolicy::new_without_delay(3))

        ; "earlier config ignored because it doesn't match host triple"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = "aarch64-apple-darwin"
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Target,
        Some(RetryPolicy::new_without_delay(2))

        ; "earlier config applied because it matches target triple"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = "x86_64-unknown-linux-gnu"
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Target,
        Some(RetryPolicy::new_without_delay(3))

        ; "earlier config ignored because it doesn't match target triple"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = 'cfg(target_os = "macos")'
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Target,
        Some(RetryPolicy::new_without_delay(2))

        ; "earlier config applied because it matches target cfg expr"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = 'cfg(target_arch = "x86_64")'
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Target,
        Some(RetryPolicy::new_without_delay(3))

        ; "earlier config ignored because it doesn't match target cfg expr"
    )]
    fn overrides_retries(
        config_contents: &str,
        build_platform: BuildPlatform,
        retries: Option<RetryPolicy>,
    ) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);
        let package_id = graph.workspace().iter().next().unwrap().id();

        let config =
            NextestConfig::from_sources(graph.workspace().root(), &graph, None, &[][..]).unwrap();
        let query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: build_platform,
            },
            test_name: "my_test",
        };
        let overrides_for = config
            .profile("ci")
            .expect("ci profile is defined")
            .apply_build_platforms(&build_platforms())
            .overrides_for(&query);
        assert_eq!(
            overrides_for.retries(),
            retries,
            "actual retries don't match expected retries"
        );
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

    #[test]
    fn parse_retries_valid() {
        let config_contents = indoc! {r#"
            [profile.default]
            retries = { backoff = "fixed", count = 3 }

            [profile.no-retries]
            retries = 0

            [profile.fixed-with-delay]
            retries = { backoff = "fixed", count = 3, delay = "1s" }

            [profile.exp]
            retries = { backoff = "exponential", count = 4, delay = "2s" }

            [profile.exp-with-max-delay]
            retries = { backoff = "exponential", count = 5, delay = "3s", max-delay = "10s" }

            [profile.exp-with-max-delay-and-jitter]
            retries = { backoff = "exponential", count = 6, delay = "4s", max-delay = "1m", jitter = true }
        "#};

        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);

        let config = NextestConfig::from_sources(graph.workspace().root(), &graph, None, [])
            .expect("config is valid");
        assert_eq!(
            config
                .profile("default")
                .expect("default profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::Fixed {
                count: 3,
                delay: Duration::ZERO,
                jitter: false,
            },
            "default retries matches"
        );

        assert_eq!(
            config
                .profile("no-retries")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::new_without_delay(0),
            "no-retries retries matches"
        );

        assert_eq!(
            config
                .profile("fixed-with-delay")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::Fixed {
                count: 3,
                delay: Duration::from_secs(1),
                jitter: false,
            },
            "fixed-with-delay retries matches"
        );

        assert_eq!(
            config
                .profile("exp")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::Exponential {
                count: 4,
                delay: Duration::from_secs(2),
                jitter: false,
                max_delay: None,
            },
            "exp retries matches"
        );

        assert_eq!(
            config
                .profile("exp-with-max-delay")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::Exponential {
                count: 5,
                delay: Duration::from_secs(3),
                jitter: false,
                max_delay: Some(Duration::from_secs(10)),
            },
            "exp-with-max-delay retries matches"
        );

        assert_eq!(
            config
                .profile("exp-with-max-delay-and-jitter")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::Exponential {
                count: 6,
                delay: Duration::from_secs(4),
                jitter: true,
                max_delay: Some(Duration::from_secs(60)),
            },
            "exp-with-max-delay-and-jitter retries matches"
        );
    }

    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "foo" }
        "#},
        "unknown variant `foo`, expected `fixed` or `exponential`"
        ; "invalid value for backoff")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed" }
        "#},
        "missing field `count`"
        ; "fixed specified without count")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed", count = 1, delay = "foobar" }
        "#},
        "invalid value: string \"foobar\", expected a duration"
        ; "delay is not a valid duration")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed", count = 1, jitter = true }
        "#},
        "`jitter` cannot be true if `delay` isn't specified or is zero"
        ; "jitter specified without delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed", count = 1, max-delay = "10s" }
        "#},
        "unknown field `max-delay`, expected one of `count`, `delay`, `jitter`"
        ; "max-delay is incompatible with fixed backoff")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1 }
        "#},
        "missing field `delay`"
        ; "exponential backoff must specify delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", delay = "1s" }
        "#},
        "missing field `count`"
        ; "exponential backoff must specify count")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 0, delay = "1s" }
        "#},
        "`count` cannot be zero with exponential backoff"
        ; "exponential backoff must have a non-zero count")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1, delay = "0s" }
        "#},
        "`delay` cannot be zero with exponential backoff"
        ; "exponential backoff must have a non-zero delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1, delay = "1s", max-delay = "0s" }
        "#},
        "`max-delay` cannot be zero with exponential backoff"
        ; "exponential backoff must have a non-zero max delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1, delay = "4s", max-delay = "2s", jitter = true }
        "#},
        "`max-delay` cannot be less than delay"
        ; "max-delay greater than delay")]
    fn parse_retries_invalid(config_contents: &str, expected_message: &str) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);

        let config_err = NextestConfig::from_sources(graph.workspace().root(), &graph, None, [])
            .expect_err("config expected to be invalid");

        let message = match config_err.kind() {
            ConfigParseErrorKind::DeserializeError(path_error) => match path_error.inner() {
                ConfigError::Message(message) => message,
                other => {
                    panic!("for config error {config_err:?}, expected ConfigError::Message for inner error {other:?}");
                }
            },
            other => {
                panic!(
                    "for config error {other:?}, expected ConfigParseErrorKind::DeserializeError"
                );
            }
        };

        assert!(
            message.contains(expected_message),
            "expected message \"{message}\" to contain \"{expected_message}\""
        );
    }

    #[test]
    fn parse_tool_config_file() {
        cfg_if::cfg_if! {
            if #[cfg(windows)] {
                let valid = ["tool:C:\\foo\\bar", "tool:\\\\?\\C:\\foo\\bar"];
                let invalid = ["C:\\foo\\bar", "tool:\\foo\\bar", "tool:", ":/foo/bar"];
            } else {
                let valid = ["tool:/foo/bar"];
                let invalid = ["/foo/bar", "tool:", ":/foo/bar", "tool:foo/bar"];
            }
        }

        for valid_input in valid {
            valid_input.parse::<ToolConfigFile>().unwrap_or_else(|err| {
                panic!("valid input {valid_input} should parse correctly: {err}")
            });
        }

        for invalid_input in invalid {
            invalid_input
                .parse::<ToolConfigFile>()
                .expect_err(&format!("invalid input {invalid_input} should error out"));
        }
    }

    #[test]
    fn tool_config_basic() {
        let config_contents = r#"
        [profile.default]
        retries = 3

        [[profile.default.overrides]]
        filter = 'test(test_foo)'
        retries = 20
        test-group = 'foo'

        [[profile.default.overrides]]
        filter = 'test(test_quux)'
        test-group = '@tool:tool1:group1'

        [test-groups.foo]
        max-threads = 2
        "#;

        let tool1_config_contents = r#"
        [profile.default]
        retries = 4

        [[profile.default.overrides]]
        filter = 'test(test_bar)'
        retries = 21

        [profile.tool]
        retries = 12

        [[profile.tool.overrides]]
        filter = 'test(test_baz)'
        retries = 22
        test-group = '@tool:tool1:group1'

        [[profile.tool.overrides]]
        filter = 'test(test_quux)'
        retries = 22
        test-group = '@tool:tool2:group2'

        [test-groups.'@tool:tool1:group1']
        max-threads = 2
        "#;

        let tool2_config_contents = r#"
        [profile.default]
        retries = 5

        [[profile.default.overrides]]
        filter = 'test(test_)'
        retries = 23

        [profile.tool]
        retries = 16

        [[profile.tool.overrides]]
        filter = 'test(test_ba)'
        retries = 24
        test-group = '@tool:tool2:group2'

        [[profile.tool.overrides]]
        filter = 'test(test_)'
        retries = 25
        test-group = '@global'

        [profile.tool2]
        retries = 18

        [[profile.tool2.overrides]]
        filter = 'all()'
        retries = 26

        [test-groups.'@tool:tool2:group2']
        max-threads = 4
        "#;

        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);
        let workspace_root = graph.workspace().root();
        let tool1_path = workspace_root.join(".config/tool1.toml");
        let tool2_path = workspace_root.join(".config/tool2.toml");
        std::fs::write(&tool1_path, tool1_config_contents).unwrap();
        std::fs::write(&tool2_path, tool2_config_contents).unwrap();

        let config = NextestConfig::from_sources(
            workspace_root,
            &graph,
            None,
            &[
                ToolConfigFile {
                    tool: "tool1".to_owned(),
                    config_file: tool1_path,
                },
                ToolConfigFile {
                    tool: "tool2".to_owned(),
                    config_file: tool2_path,
                },
            ],
        )
        .expect("config is valid");

        let default_profile = config
            .profile(NextestConfig::DEFAULT_PROFILE)
            .expect("default profile is present")
            .apply_build_platforms(&build_platforms());
        // This is present in .config/nextest.toml and is the highest priority
        assert_eq!(default_profile.retries(), RetryPolicy::new_without_delay(3));

        let package_id = graph.workspace().iter().next().unwrap().id();

        let test_foo_query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: BuildPlatform::Target,
            },
            test_name: "test_foo",
        };
        let test_bar_query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: BuildPlatform::Target,
            },
            test_name: "test_bar",
        };
        let test_baz_query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: BuildPlatform::Target,
            },
            test_name: "test_baz",
        };
        let test_quux_query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: BuildPlatform::Target,
            },
            test_name: "test_quux",
        };

        assert_eq!(
            default_profile.overrides_for(&test_foo_query).retries(),
            Some(RetryPolicy::new_without_delay(20)),
            "retries for test_foo/default profile"
        );
        assert_eq!(
            default_profile.overrides_for(&test_foo_query).test_group(),
            Some(&test_group("foo")),
            "test_group for test_foo/default profile"
        );
        assert_eq!(
            default_profile.overrides_for(&test_bar_query).retries(),
            Some(RetryPolicy::new_without_delay(21)),
            "retries for test_bar/default profile"
        );
        assert_eq!(
            default_profile.overrides_for(&test_bar_query).test_group(),
            None,
            "test_group for test_bar/default profile"
        );
        assert_eq!(
            default_profile.overrides_for(&test_baz_query).retries(),
            Some(RetryPolicy::new_without_delay(23)),
            "retries for test_baz/default profile"
        );
        assert_eq!(
            default_profile.overrides_for(&test_quux_query).test_group(),
            Some(&test_group("@tool:tool1:group1")),
            "test group for test_quux/default profile"
        );

        let tool_profile = config
            .profile("tool")
            .expect("tool profile is present")
            .apply_build_platforms(&build_platforms());
        assert_eq!(tool_profile.retries(), RetryPolicy::new_without_delay(12));
        assert_eq!(
            tool_profile.overrides_for(&test_foo_query).retries(),
            Some(RetryPolicy::new_without_delay(25)),
            "retries for test_foo/default profile"
        );
        assert_eq!(
            tool_profile.overrides_for(&test_bar_query).retries(),
            Some(RetryPolicy::new_without_delay(24)),
            "retries for test_bar/default profile"
        );
        assert_eq!(
            tool_profile.overrides_for(&test_baz_query).retries(),
            Some(RetryPolicy::new_without_delay(22)),
            "retries for test_baz/default profile"
        );

        let tool2_profile = config
            .profile("tool2")
            .expect("tool2 profile is present")
            .apply_build_platforms(&build_platforms());
        assert_eq!(tool2_profile.retries(), RetryPolicy::new_without_delay(18));
        assert_eq!(
            tool2_profile.overrides_for(&test_foo_query).retries(),
            Some(RetryPolicy::new_without_delay(26)),
            "retries for test_foo/default profile"
        );
        assert_eq!(
            tool2_profile.overrides_for(&test_bar_query).retries(),
            Some(RetryPolicy::new_without_delay(26)),
            "retries for test_bar/default profile"
        );
        assert_eq!(
            tool2_profile.overrides_for(&test_baz_query).retries(),
            Some(RetryPolicy::new_without_delay(26)),
            "retries for test_baz/default profile"
        );
    }

    #[derive(Debug)]
    enum GroupExpectedError {
        DeserializeError(&'static str),
        InvalidTestGroups(BTreeSet<CustomTestGroup>),
    }

    #[test_case(
        indoc!{r#"
            [test-groups."@tool:my-tool:foo"]
            max-threads = 1
        "#},
        Ok(btreeset! {custom_test_group("user-group"), custom_test_group("@tool:my-tool:foo")})
        ; "group name valid")]
    #[test_case(
        indoc!{r#"
            [test-groups.foo]
            max-threads = 1
        "#},
        Err(GroupExpectedError::InvalidTestGroups(btreeset! {custom_test_group("foo")}))
        ; "group name doesn't start with @tool:")]
    #[test_case(
        indoc!{r#"
            [test-groups."@tool:moo:test"]
            max-threads = 1
        "#},
        Err(GroupExpectedError::InvalidTestGroups(btreeset! {custom_test_group("@tool:moo:test")}))
        ; "group name doesn't start with tool name")]
    #[test_case(
        indoc!{r#"
            [test-groups."@tool:my-tool"]
            max-threads = 1
        "#},
        Err(GroupExpectedError::DeserializeError("test-groups.@tool:my-tool: invalid custom test group name: tool identifier not of the form \"@tool:tool-name:identifier\": `@tool:my-tool`"))
        ; "group name missing suffix colon")]
    #[test_case(
        indoc!{r#"
            [test-groups.'@global']
            max-threads = 1
        "#},
        Err(GroupExpectedError::DeserializeError("test-groups.@global: invalid custom test group name: invalid identifier `@global`"))
        ; "group name is @global")]
    #[test_case(
        indoc!{r#"
            [test-groups.'@foo']
            max-threads = 1
        "#},
        Err(GroupExpectedError::DeserializeError("test-groups.@foo: invalid custom test group name: invalid identifier `@foo`"))
        ; "group name starts with @")]
    fn tool_config_define_groups(
        input: &str,
        expected: Result<BTreeSet<CustomTestGroup>, GroupExpectedError>,
    ) {
        let config_contents = indoc! {r#"
            [profile.default]
            test-group = "user-group"

            [test-groups.user-group]
            max-threads = 1
        "#};
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);
        let workspace_root = graph.workspace().root();
        let tool_path = workspace_root.join(".config/tool.toml");
        std::fs::write(&tool_path, input).unwrap();

        let config_res = NextestConfig::from_sources(
            workspace_root,
            &graph,
            None,
            &[ToolConfigFile {
                tool: "my-tool".to_owned(),
                config_file: tool_path.clone(),
            }][..],
        );
        match expected {
            Ok(expected_groups) => {
                let config = config_res.expect("config is valid");
                let profile = config.profile("default").expect("default profile is known");
                let profile = profile.apply_build_platforms(&build_platforms());
                assert_eq!(
                    profile
                        .test_group_config()
                        .keys()
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                    expected_groups
                );
            }
            Err(expected_error) => {
                let error = config_res.expect_err("config is invalid");
                assert_eq!(error.config_file(), &tool_path);
                assert_eq!(error.tool(), Some("my-tool"));
                match &expected_error {
                    GroupExpectedError::InvalidTestGroups(expected_groups) => {
                        assert!(
                            matches!(
                                error.kind(),
                                ConfigParseErrorKind::InvalidTestGroupsDefinedByTool(groups)
                                    if groups == expected_groups
                            ),
                            "expected config.kind ({}) to be {:?}",
                            error.kind(),
                            expected_error,
                        );
                    }
                    GroupExpectedError::DeserializeError(error_str) => {
                        assert!(
                            matches!(
                                error.kind(),
                                ConfigParseErrorKind::DeserializeError(error)
                                    if error.to_string() == *error_str
                            ),
                            "expected config.kind ({}) to be {:?}",
                            error.kind(),
                            expected_error,
                        );
                    }
                }
            }
        }
    }

    #[test_case(
        indoc!{r#"
            [test-groups."my-group"]
            max-threads = 1
        "#},
        Ok(btreeset! {custom_test_group("my-group")})
        ; "group name valid")]
    #[test_case(
        indoc!{r#"
            [test-groups."@tool:"]
            max-threads = 1
        "#},
        Err(GroupExpectedError::DeserializeError("test-groups.@tool:: invalid custom test group name: tool identifier not of the form \"@tool:tool-name:identifier\": `@tool:`"))
        ; "group name starts with @tool:")]
    #[test_case(
        indoc!{r#"
            [test-groups.'@global']
            max-threads = 1
        "#},
        Err(GroupExpectedError::DeserializeError("test-groups.@global: invalid custom test group name: invalid identifier `@global`"))
        ; "group name is @global")]
    #[test_case(
        indoc!{r#"
            [test-groups.'@foo']
            max-threads = 1
        "#},
        Err(GroupExpectedError::DeserializeError("test-groups.@foo: invalid custom test group name: invalid identifier `@foo`"))
        ; "group name starts with @")]
    fn user_config_define_groups(
        config_contents: &str,
        expected: Result<BTreeSet<CustomTestGroup>, GroupExpectedError>,
    ) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);
        let workspace_root = graph.workspace().root();

        let config_res = NextestConfig::from_sources(workspace_root, &graph, None, &[][..]);
        match expected {
            Ok(expected_groups) => {
                let config = config_res.expect("config is valid");
                let profile = config.profile("default").expect("default profile is known");
                let profile = profile.apply_build_platforms(&build_platforms());
                assert_eq!(
                    profile
                        .test_group_config()
                        .keys()
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                    expected_groups
                );
            }
            Err(expected_error) => {
                let error = config_res.expect_err("config is invalid");
                assert_eq!(error.tool(), None);
                match &expected_error {
                    GroupExpectedError::InvalidTestGroups(expected_groups) => {
                        assert!(
                            matches!(
                                error.kind(),
                                ConfigParseErrorKind::InvalidTestGroupsDefined(groups)
                                    if groups == expected_groups
                            ),
                            "expected config.kind ({}) to be {:?}",
                            error.kind(),
                            expected_error,
                        );
                    }
                    GroupExpectedError::DeserializeError(error_str) => {
                        assert!(
                            matches!(
                                error.kind(),
                                ConfigParseErrorKind::DeserializeError(error)
                                    if error.to_string() == *error_str
                            ),
                            "expected config.kind ({}) to be {:?}",
                            error.kind(),
                            expected_error,
                        );
                    }
                }
            }
        }
    }

    fn test_group(name: &str) -> TestGroup {
        TestGroup::Custom(custom_test_group(name))
    }

    fn custom_test_group(name: &str) -> CustomTestGroup {
        CustomTestGroup::new(name.into())
            .unwrap_or_else(|error| panic!("invalid custom test group {name}: {error}"))
    }

    #[test_case(
        indoc!{r#"
            [[profile.default.overrides]]
            filter = 'all()'
            test-group = "foo"
        "#},
        "",
        "",
        Some("tool1"),
        vec![UnknownTestGroupError {
            profile_name: "default".to_owned(),
            name: test_group("foo"),
        }],
        btreeset! { TestGroup::Global }
        ; "unknown group in tool config")]
    #[test_case(
        "",
        "",
        indoc!{r#"
            [[profile.default.overrides]]
            filter = 'all()'
            test-group = "foo"
        "#},
        None,
        vec![UnknownTestGroupError {
            profile_name: "default".to_owned(),
            name: test_group("foo"),
        }],
        btreeset! { TestGroup::Global }
        ; "unknown group in user config")]
    #[test_case(
        indoc!{r#"
            [[profile.default.overrides]]
            filter = 'all()'
            test-group = "@tool:tool1:foo"

            [test-groups."@tool:tool1:foo"]
            max-threads = 1
        "#},
        indoc!{r#"
            [[profile.default.overrides]]
            filter = 'all()'
            test-group = "@tool:tool1:foo"
        "#},
        indoc!{r#"
            [[profile.default.overrides]]
            filter = 'all()'
            test-group = "foo"
        "#},
        Some("tool2"),
        vec![UnknownTestGroupError {
            profile_name: "default".to_owned(),
            name: test_group("@tool:tool1:foo"),
        }],
        btreeset! { TestGroup::Global }
        ; "depends on downstream tool config")]
    #[test_case(
        indoc!{r#"
            [[profile.default.overrides]]
            filter = 'all()'
            test-group = "foo"
        "#},
        "",
        indoc!{r#"
            [[profile.default.overrides]]
            filter = 'all()'
            test-group = "foo"

            [test-groups.foo]
            max-threads = 1
        "#},
        Some("tool1"),
        vec![UnknownTestGroupError {
            profile_name: "default".to_owned(),
            name: test_group("foo"),
        }],
        btreeset! { TestGroup::Global }
        ; "depends on user config")]
    fn unknown_groups(
        tool1_config: &str,
        tool2_config: &str,
        user_config: &str,
        tool: Option<&str>,
        expected_errors: Vec<UnknownTestGroupError>,
        expected_known_groups: BTreeSet<TestGroup>,
    ) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, user_config);
        let workspace_root = graph.workspace().root();
        let tool1_path = workspace_root.join(".config/tool1.toml");
        std::fs::write(&tool1_path, tool1_config).unwrap();
        let tool2_path = workspace_root.join(".config/tool2.toml");
        std::fs::write(&tool2_path, tool2_config).unwrap();

        let config = NextestConfig::from_sources(
            workspace_root,
            &graph,
            None,
            &[
                ToolConfigFile {
                    tool: "tool1".to_owned(),
                    config_file: tool1_path,
                },
                ToolConfigFile {
                    tool: "tool2".to_owned(),
                    config_file: tool2_path,
                },
            ][..],
        )
        .expect_err("config is invalid");
        assert_eq!(config.tool(), tool);
        match config.kind() {
            ConfigParseErrorKind::UnknownTestGroups {
                errors,
                known_groups,
            } => {
                assert_eq!(errors, &expected_errors, "expected errors match");
                assert_eq!(known_groups, &expected_known_groups, "known groups match");
            }
            other => {
                panic!("expected ConfigParseErrorKind::UnknownTestGroups, got {other}");
            }
        }
    }

    #[test]
    fn ignored_keys() {
        let config_contents = r#"
        ignored1 = "test"

        [profile.default]
        retries = 3
        ignored2 = "hi"

        [[profile.default.overrides]]
        filter = 'test(test_foo)'
        retries = 20
        ignored3 = 42
        "#;

        let tool_config_contents = r#"
        [store]
        ignored4 = 20

        [profile.default]
        retries = 4
        ignored5 = false

        [profile.tool]
        retries = 12

        [[profile.tool.overrides]]
        filter = 'test(test_baz)'
        retries = 22
        ignored6 = 6.5
        "#;

        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);
        let workspace_root = graph.workspace().root();
        let tool_path = workspace_root.join(".config/tool.toml");
        std::fs::write(&tool_path, tool_config_contents).unwrap();

        let mut unknown_keys = HashMap::new();

        let _ = NextestConfig::from_sources_impl(
            workspace_root,
            &graph,
            None,
            &[ToolConfigFile {
                tool: "my-tool".to_owned(),
                config_file: tool_path,
            }][..],
            |_path, tool, ignored| {
                unknown_keys.insert(tool.map(|s| s.to_owned()), ignored.clone());
            },
        )
        .expect("config is valid");

        assert_eq!(
            unknown_keys.len(),
            2,
            "there are two files with unknown keys"
        );

        let keys = unknown_keys
            .remove(&None)
            .expect("unknown keys for .config/nextest.toml");
        assert_eq!(
            keys,
            maplit::btreeset! {
                "ignored1".to_owned(),
                "profile.default.ignored2".to_owned(),
                "profile.default.overrides.0.ignored3".to_owned(),
            }
        );

        let keys = unknown_keys
            .remove(&Some("my-tool".to_owned()))
            .expect("unknown keys for my-tool");
        assert_eq!(
            keys,
            maplit::btreeset! {
                "store.ignored4".to_owned(),
                "profile.default.ignored5".to_owned(),
                "profile.tool.overrides.0.ignored6".to_owned(),
            }
        );
    }

    #[test_case(
        indoc! {r#"
            [profile.custom]
            test-threads = -1
        "#},
        Some(get_num_cpus() - 1)

        ; "negative"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            test-threads = 2
        "#},
        Some(2)

        ; "positive"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            test-threads = 0
        "#},
        None

        ; "zero"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            test-threads = "num-cpus"
        "#},
        Some(get_num_cpus())

        ; "num-cpus"
    )]
    fn parse_test_threads(config_contents: &str, n_threads: Option<usize>) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);

        let config = NextestConfig::from_sources(graph.workspace().root(), &graph, None, []);
        match n_threads {
            None => assert!(config.is_err()),
            Some(n) => assert_eq!(
                config
                    .unwrap()
                    .profile("custom")
                    .unwrap()
                    .apply_build_platforms(&build_platforms())
                    .custom_profile
                    .unwrap()
                    .test_threads
                    .unwrap()
                    .compute(),
                n,
            ),
        }
    }


}
