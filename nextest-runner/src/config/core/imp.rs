// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{ExperimentalDeserialize, NextestVersionDeserialize, ToolConfigFile, ToolName};
use crate::{
    config::{
        core::ConfigExperimental,
        elements::{
            ArchiveConfig, BenchConfig, CustomTestGroup, DefaultBenchConfig, DefaultJunitImpl,
            GlobalTimeout, Inherits, JunitConfig, JunitImpl, JunitSettings, LeakTimeout, MaxFail,
            RetryPolicy, SlowTimeout, TestGroup, TestGroupConfig, TestThreads, ThreadsRequired,
            deserialize_fail_fast, deserialize_leak_timeout, deserialize_retry_policy,
            deserialize_slow_timeout,
        },
        overrides::{
            CompiledByProfile, CompiledData, CompiledDefaultFilter, DeserializedOverride,
            ListSettings, SettingSource, TestSettings,
        },
        scripts::{
            DeserializedProfileScriptConfig, ProfileScriptType, ScriptConfig, ScriptId, ScriptInfo,
            SetupScriptConfig, SetupScripts,
        },
    },
    errors::{
        ConfigParseError, ConfigParseErrorKind, InheritsError,
        ProfileListScriptUsesRunFiltersError, ProfileNotFound, ProfileScriptErrors,
        ProfileUnknownScriptError, ProfileWrongConfigScriptTypeError, UnknownTestGroupError,
        provided_by_tool,
    },
    helpers::plural,
    list::TestList,
    platform::BuildPlatforms,
    reporter::{FinalStatusLevel, StatusLevel, TestOutputDisplay},
    run_mode::NextestRunMode,
};
use camino::{Utf8Path, Utf8PathBuf};
use config::{
    Config, ConfigBuilder, ConfigError, File, FileFormat, FileSourceFile, builder::DefaultState,
};
use iddqd::IdOrdMap;
use indexmap::IndexMap;
use nextest_filtering::{BinaryQuery, EvalContext, Filterset, ParseContext, TestQuery};
use petgraph::{Directed, Graph, algo::scc::kosaraju_scc, graph::NodeIndex};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, hash_map},
    sync::LazyLock,
};
use tracing::warn;

/// Trait for handling configuration warnings.
///
/// This trait allows for different warning handling strategies, such as logging warnings
/// (the default behavior) or collecting them for testing purposes.
pub trait ConfigWarnings {
    /// Handle unknown configuration keys found in a config file.
    fn unknown_config_keys(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&ToolName>,
        unknown: &BTreeSet<String>,
    );

    /// Handle unknown profiles found in the reserved `default-` namespace.
    fn unknown_reserved_profiles(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&ToolName>,
        profiles: &[&str],
    );

    /// Handle deprecated `[script.*]` configuration.
    fn deprecated_script_config(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&ToolName>,
    );

    /// Handle warning about empty script sections with neither setup nor
    /// wrapper scripts.
    fn empty_script_sections(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&ToolName>,
        profile_name: &str,
        empty_count: usize,
    );
}

/// Default implementation of ConfigWarnings that logs warnings using the tracing crate.
pub struct DefaultConfigWarnings;

impl ConfigWarnings for DefaultConfigWarnings {
    fn unknown_config_keys(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&ToolName>,
        unknown: &BTreeSet<String>,
    ) {
        let mut unknown_str = String::new();
        if unknown.len() == 1 {
            // Print this on the same line.
            unknown_str.push_str("key: ");
            unknown_str.push_str(unknown.iter().next().unwrap());
        } else {
            unknown_str.push_str("keys:\n");
            for ignored_key in unknown {
                unknown_str.push('\n');
                unknown_str.push_str("  - ");
                unknown_str.push_str(ignored_key);
            }
        }

        warn!(
            "in config file {}{}, ignoring unknown configuration {unknown_str}",
            config_file
                .strip_prefix(workspace_root)
                .unwrap_or(config_file),
            provided_by_tool(tool),
        )
    }

    fn unknown_reserved_profiles(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&ToolName>,
        profiles: &[&str],
    ) {
        warn!(
            "in config file {}{}, ignoring unknown profiles in the reserved `default-` namespace:",
            config_file
                .strip_prefix(workspace_root)
                .unwrap_or(config_file),
            provided_by_tool(tool),
        );

        for profile in profiles {
            warn!("  {profile}");
        }
    }

    fn deprecated_script_config(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&ToolName>,
    ) {
        warn!(
            "in config file {}{}, [script.*] is deprecated and will be removed in a \
             future version of nextest; use the `scripts.setup` table instead",
            config_file
                .strip_prefix(workspace_root)
                .unwrap_or(config_file),
            provided_by_tool(tool),
        );
    }

    fn empty_script_sections(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&ToolName>,
        profile_name: &str,
        empty_count: usize,
    ) {
        warn!(
            "in config file {}{}, [[profile.{}.scripts]] has {} {} \
             with neither setup nor wrapper scripts",
            config_file
                .strip_prefix(workspace_root)
                .unwrap_or(config_file),
            provided_by_tool(tool),
            profile_name,
            empty_count,
            plural::sections_str(empty_count),
        );
    }
}

/// Gets the number of available CPUs and caches the value.
#[inline]
pub fn get_num_cpus() -> usize {
    static NUM_CPUS: LazyLock<usize> =
        LazyLock::new(|| match std::thread::available_parallelism() {
            Ok(count) => count.into(),
            Err(err) => {
                warn!("unable to determine num-cpus ({err}), assuming 1 logical CPU");
                1
            }
        });

    *NUM_CPUS
}

/// Overall configuration for nextest.
///
/// This is the root data structure for nextest configuration. Most runner-specific configuration is
/// managed through [profiles](EvaluatableProfile), obtained through the [`profile`](Self::profile)
/// method.
///
/// For more about configuration, see [_Configuration_](https://nexte.st/docs/configuration) in the
/// nextest book.
#[derive(Clone, Debug)]
pub struct NextestConfig {
    workspace_root: Utf8PathBuf,
    inner: NextestConfigImpl,
    compiled: CompiledByProfile,
}

impl NextestConfig {
    /// The default location of the config within the path: `.config/nextest.toml`, used to read the
    /// config from the given directory.
    pub const CONFIG_PATH: &'static str = ".config/nextest.toml";

    /// Contains the default config as a TOML file.
    ///
    /// Repository-specific configuration is layered on top of the default config.
    pub const DEFAULT_CONFIG: &'static str = include_str!("../../../default-config.toml");

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
        pcx: &ParseContext<'_>,
        config_file: Option<&Utf8Path>,
        tool_config_files: impl IntoIterator<IntoIter = I>,
        experimental: &BTreeSet<ConfigExperimental>,
    ) -> Result<Self, ConfigParseError>
    where
        I: Iterator<Item = &'a ToolConfigFile> + DoubleEndedIterator,
    {
        Self::from_sources_with_warnings(
            workspace_root,
            pcx,
            config_file,
            tool_config_files,
            experimental,
            &mut DefaultConfigWarnings,
        )
    }

    /// Load configuration from the given sources with custom warning handling.
    pub fn from_sources_with_warnings<'a, I>(
        workspace_root: impl Into<Utf8PathBuf>,
        pcx: &ParseContext<'_>,
        config_file: Option<&Utf8Path>,
        tool_config_files: impl IntoIterator<IntoIter = I>,
        experimental: &BTreeSet<ConfigExperimental>,
        warnings: &mut impl ConfigWarnings,
    ) -> Result<Self, ConfigParseError>
    where
        I: Iterator<Item = &'a ToolConfigFile> + DoubleEndedIterator,
    {
        Self::from_sources_impl(
            workspace_root,
            pcx,
            config_file,
            tool_config_files,
            experimental,
            warnings,
        )
    }

    // A custom unknown_callback can be passed in while testing.
    fn from_sources_impl<'a, I>(
        workspace_root: impl Into<Utf8PathBuf>,
        pcx: &ParseContext<'_>,
        config_file: Option<&Utf8Path>,
        tool_config_files: impl IntoIterator<IntoIter = I>,
        experimental: &BTreeSet<ConfigExperimental>,
        warnings: &mut impl ConfigWarnings,
    ) -> Result<Self, ConfigParseError>
    where
        I: Iterator<Item = &'a ToolConfigFile> + DoubleEndedIterator,
    {
        let workspace_root = workspace_root.into();
        let tool_config_files_rev = tool_config_files.into_iter().rev();
        let (inner, compiled) = Self::read_from_sources(
            pcx,
            &workspace_root,
            config_file,
            tool_config_files_rev,
            experimental,
            warnings,
        )?;
        Ok(Self {
            workspace_root,
            inner,
            compiled,
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
            // The default config has no overrides or special settings.
            compiled: CompiledByProfile::for_default_config(),
        }
    }

    /// Returns the profile with the given name, or an error if a profile was
    /// specified but not found.
    pub fn profile(&self, name: impl AsRef<str>) -> Result<EarlyProfile<'_>, ProfileNotFound> {
        self.make_profile(name.as_ref())
    }

    // ---
    // Helper methods
    // ---

    fn read_from_sources<'a>(
        pcx: &ParseContext<'_>,
        workspace_root: &Utf8Path,
        file: Option<&Utf8Path>,
        tool_config_files_rev: impl Iterator<Item = &'a ToolConfigFile>,
        experimental: &BTreeSet<ConfigExperimental>,
        warnings: &mut impl ConfigWarnings,
    ) -> Result<(NextestConfigImpl, CompiledByProfile), ConfigParseError> {
        // First, get the default config.
        let mut composite_builder = Self::make_default_config();

        // Overrides are handled additively.
        // Note that they're stored in reverse order here, and are flipped over at the end.
        let mut compiled = CompiledByProfile::for_default_config();

        let mut known_groups = BTreeSet::new();
        let mut known_scripts = IdOrdMap::new();
        // Track known profiles for inheritance validation. Profiles can only inherit
        // from profiles defined in the same file or in previously loaded (lower priority) files.
        let mut known_profiles = BTreeSet::new();

        // Next, merge in tool configs.
        for ToolConfigFile { config_file, tool } in tool_config_files_rev {
            let source = File::new(config_file.as_str(), FileFormat::Toml);
            Self::deserialize_individual_config(
                pcx,
                workspace_root,
                config_file,
                Some(tool),
                source.clone(),
                &mut compiled,
                experimental,
                warnings,
                &mut known_groups,
                &mut known_scripts,
                &mut known_profiles,
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
            pcx,
            workspace_root,
            &config_file,
            None,
            source.clone(),
            &mut compiled,
            experimental,
            warnings,
            &mut known_groups,
            &mut known_scripts,
            &mut known_profiles,
        )?;

        composite_builder = composite_builder.add_source(source);

        // The unknown set is ignored here because any values in it have already been reported in
        // deserialize_individual_config.
        let (config, _unknown) = Self::build_and_deserialize_config(&composite_builder)
            .map_err(|kind| ConfigParseError::new(&config_file, None, kind))?;

        // Reverse all the compiled data at the end.
        compiled.default.reverse();
        for data in compiled.other.values_mut() {
            data.reverse();
        }

        Ok((config.into_config_impl(), compiled))
    }

    #[expect(clippy::too_many_arguments)]
    fn deserialize_individual_config(
        pcx: &ParseContext<'_>,
        workspace_root: &Utf8Path,
        config_file: &Utf8Path,
        tool: Option<&ToolName>,
        source: File<FileSourceFile, FileFormat>,
        compiled_out: &mut CompiledByProfile,
        experimental: &BTreeSet<ConfigExperimental>,
        warnings: &mut impl ConfigWarnings,
        known_groups: &mut BTreeSet<CustomTestGroup>,
        known_scripts: &mut IdOrdMap<ScriptInfo>,
        known_profiles: &mut BTreeSet<String>,
    ) -> Result<(), ConfigParseError> {
        // Try building default builder + this file to get good error attribution and handle
        // overrides additively.
        let default_builder = Self::make_default_config();
        let this_builder = default_builder.add_source(source);
        let (mut this_config, unknown) = Self::build_and_deserialize_config(&this_builder)
            .map_err(|kind| ConfigParseError::new(config_file, tool, kind))?;

        if !unknown.is_empty() {
            warnings.unknown_config_keys(config_file, workspace_root, tool, &unknown);
        }

        // Check that test groups are named as expected.
        let (valid_groups, invalid_groups): (BTreeSet<_>, _) =
            this_config.test_groups.keys().cloned().partition(|group| {
                if let Some(tool) = tool {
                    // The first component must be the tool name.
                    group
                        .as_identifier()
                        .tool_components()
                        .is_some_and(|(tool_name, _)| tool_name == tool.as_str())
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

        // If both scripts and old_setup_scripts are present, produce an error.
        if !this_config.scripts.is_empty() && !this_config.old_setup_scripts.is_empty() {
            return Err(ConfigParseError::new(
                config_file,
                tool,
                ConfigParseErrorKind::BothScriptAndScriptsDefined,
            ));
        }

        // If old_setup_scripts are present, produce a warning.
        if !this_config.old_setup_scripts.is_empty() {
            warnings.deprecated_script_config(config_file, workspace_root, tool);
            this_config.scripts.setup = this_config.old_setup_scripts.clone();
        }

        // Check for experimental features that are used but not enabled.
        {
            let mut missing_features = BTreeSet::new();
            if !this_config.scripts.setup.is_empty()
                && !experimental.contains(&ConfigExperimental::SetupScripts)
            {
                missing_features.insert(ConfigExperimental::SetupScripts);
            }
            if !this_config.scripts.wrapper.is_empty()
                && !experimental.contains(&ConfigExperimental::WrapperScripts)
            {
                missing_features.insert(ConfigExperimental::WrapperScripts);
            }
            if !missing_features.is_empty() {
                return Err(ConfigParseError::new(
                    config_file,
                    tool,
                    ConfigParseErrorKind::ExperimentalFeaturesNotEnabled { missing_features },
                ));
            }
        }

        let duplicate_ids: BTreeSet<_> = this_config.scripts.duplicate_ids().cloned().collect();
        if !duplicate_ids.is_empty() {
            return Err(ConfigParseError::new(
                config_file,
                tool,
                ConfigParseErrorKind::DuplicateConfigScriptNames(duplicate_ids),
            ));
        }

        // Check that setup scripts are named as expected.
        let (valid_scripts, invalid_scripts): (BTreeSet<_>, _) = this_config
            .scripts
            .all_script_ids()
            .cloned()
            .partition(|script| {
                if let Some(tool) = tool {
                    // The first component must be the tool name.
                    script
                        .as_identifier()
                        .tool_components()
                        .is_some_and(|(tool_name, _)| tool_name == tool.as_str())
                } else {
                    // If a tool is not specified, it must *not* be a tool identifier.
                    !script.as_identifier().is_tool_identifier()
                }
            });

        if !invalid_scripts.is_empty() {
            let kind = if tool.is_some() {
                ConfigParseErrorKind::InvalidConfigScriptsDefinedByTool(invalid_scripts)
            } else {
                ConfigParseErrorKind::InvalidConfigScriptsDefined(invalid_scripts)
            };
            return Err(ConfigParseError::new(config_file, tool, kind));
        }

        known_scripts.extend(
            valid_scripts
                .into_iter()
                .map(|id| this_config.scripts.script_info(id)),
        );

        let this_config = this_config.into_config_impl();

        let unknown_default_profiles: Vec<_> = this_config
            .all_profiles()
            .filter(|p| p.starts_with("default-") && !NextestConfig::DEFAULT_PROFILES.contains(p))
            .collect();
        if !unknown_default_profiles.is_empty() {
            warnings.unknown_reserved_profiles(
                config_file,
                workspace_root,
                tool,
                &unknown_default_profiles,
            );
        }

        // Check that the profiles correctly use the inherits setting.
        // Profiles can only inherit from profiles in the same file or in previously
        // loaded (lower priority) files.
        this_config
            .sanitize_profile_inherits(known_profiles)
            .map_err(|kind| ConfigParseError::new(config_file, tool, kind))?;

        // Add this file's profiles to known_profiles for subsequent files.
        known_profiles.extend(
            this_config
                .other_profiles()
                .map(|(name, _)| name.to_owned()),
        );

        // Compile the overrides for this file.
        let this_compiled = CompiledByProfile::new(pcx, &this_config)
            .map_err(|kind| ConfigParseError::new(config_file, tool, kind))?;

        // Check that all overrides specify known test groups.
        let mut unknown_group_errors = Vec::new();
        let mut check_test_group = |profile_name: &str, test_group: Option<&TestGroup>| {
            if let Some(TestGroup::Custom(group)) = test_group
                && !known_groups.contains(group)
            {
                unknown_group_errors.push(UnknownTestGroupError {
                    profile_name: profile_name.to_owned(),
                    name: TestGroup::Custom(group.clone()),
                });
            }
        };

        this_compiled
            .default
            .overrides
            .iter()
            .for_each(|override_| {
                check_test_group("default", override_.data.test_group.as_ref());
            });

        // Check that override test groups are known.
        this_compiled.other.iter().for_each(|(profile_name, data)| {
            data.overrides.iter().for_each(|override_| {
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

        // Check that scripts are known and that there aren't any other errors
        // with them.
        let mut profile_script_errors = ProfileScriptErrors::default();
        let mut check_script_ids = |profile_name: &str,
                                    script_type: ProfileScriptType,
                                    expr: Option<&Filterset>,
                                    scripts: &[ScriptId]| {
            for script in scripts {
                if let Some(script_info) = known_scripts.get(script) {
                    if !script_info.script_type.matches(script_type) {
                        profile_script_errors.wrong_script_types.push(
                            ProfileWrongConfigScriptTypeError {
                                profile_name: profile_name.to_owned(),
                                name: script.clone(),
                                attempted: script_type,
                                actual: script_info.script_type,
                            },
                        );
                    }
                    if script_type == ProfileScriptType::ListWrapper
                        && let Some(expr) = expr
                    {
                        let runtime_only_leaves = expr.parsed.runtime_only_leaves();
                        if !runtime_only_leaves.is_empty() {
                            let filters = runtime_only_leaves
                                .iter()
                                .map(|leaf| leaf.to_string())
                                .collect();
                            profile_script_errors.list_scripts_using_run_filters.push(
                                ProfileListScriptUsesRunFiltersError {
                                    profile_name: profile_name.to_owned(),
                                    name: script.clone(),
                                    script_type,
                                    filters,
                                },
                            );
                        }
                    }
                } else {
                    profile_script_errors
                        .unknown_scripts
                        .push(ProfileUnknownScriptError {
                            profile_name: profile_name.to_owned(),
                            name: script.clone(),
                        });
                }
            }
        };

        let mut empty_script_count = 0;

        this_compiled.default.scripts.iter().for_each(|scripts| {
            if scripts.setup.is_empty()
                && scripts.list_wrapper.is_none()
                && scripts.run_wrapper.is_none()
            {
                empty_script_count += 1;
            }

            check_script_ids(
                "default",
                ProfileScriptType::Setup,
                scripts.data.expr(),
                &scripts.setup,
            );
            check_script_ids(
                "default",
                ProfileScriptType::ListWrapper,
                scripts.data.expr(),
                scripts.list_wrapper.as_slice(),
            );
            check_script_ids(
                "default",
                ProfileScriptType::RunWrapper,
                scripts.data.expr(),
                scripts.run_wrapper.as_slice(),
            );
        });

        if empty_script_count > 0 {
            warnings.empty_script_sections(
                config_file,
                workspace_root,
                tool,
                "default",
                empty_script_count,
            );
        }

        this_compiled.other.iter().for_each(|(profile_name, data)| {
            let mut empty_script_count = 0;
            data.scripts.iter().for_each(|scripts| {
                if scripts.setup.is_empty()
                    && scripts.list_wrapper.is_none()
                    && scripts.run_wrapper.is_none()
                {
                    empty_script_count += 1;
                }

                check_script_ids(
                    profile_name,
                    ProfileScriptType::Setup,
                    scripts.data.expr(),
                    &scripts.setup,
                );
                check_script_ids(
                    profile_name,
                    ProfileScriptType::ListWrapper,
                    scripts.data.expr(),
                    scripts.list_wrapper.as_slice(),
                );
                check_script_ids(
                    profile_name,
                    ProfileScriptType::RunWrapper,
                    scripts.data.expr(),
                    scripts.run_wrapper.as_slice(),
                );
            });

            if empty_script_count > 0 {
                warnings.empty_script_sections(
                    config_file,
                    workspace_root,
                    tool,
                    profile_name,
                    empty_script_count,
                );
            }
        });

        // If there were any errors parsing profile-specific script data, error
        // out.
        if !profile_script_errors.is_empty() {
            let known_scripts = known_scripts
                .iter()
                .map(|script| script.id.clone())
                .collect();
            return Err(ConfigParseError::new(
                config_file,
                tool,
                ConfigParseErrorKind::ProfileScriptErrors {
                    errors: Box::new(profile_script_errors),
                    known_scripts,
                },
            ));
        }

        // Grab the compiled data (default-filter, overrides and setup scripts) for this config,
        // adding them in reversed order (we'll flip it around at the end).
        compiled_out.default.extend_reverse(this_compiled.default);
        for (name, mut data) in this_compiled.other {
            match compiled_out.other.entry(name) {
                hash_map::Entry::Vacant(entry) => {
                    // When inserting a new element, reverse the data.
                    data.reverse();
                    entry.insert(data);
                }
                hash_map::Entry::Occupied(mut entry) => {
                    // When appending to an existing element, extend the data in reverse.
                    entry.get_mut().extend_reverse(data);
                }
            }
        }

        Ok(())
    }

    fn make_default_config() -> ConfigBuilder<DefaultState> {
        Config::builder().add_source(File::from_str(Self::DEFAULT_CONFIG, FileFormat::Toml))
    }

    fn make_profile(&self, name: &str) -> Result<EarlyProfile<'_>, ProfileNotFound> {
        let custom_profile = self.inner.get_profile(name)?;

        // Resolve the inherited profile into a profile chain
        let inheritance_chain = self.inner.resolve_inheritance_chain(name)?;

        // The profile was found: construct it.
        let mut store_dir = self.workspace_root.join(&self.inner.store.dir);
        store_dir.push(name);

        // Grab the compiled data as well.
        let compiled_data = match self.compiled.other.get(name) {
            Some(data) => data.clone().chain(self.compiled.default.clone()),
            None => self.compiled.default.clone(),
        };

        Ok(EarlyProfile {
            name: name.to_owned(),
            store_dir,
            default_profile: &self.inner.default_profile,
            custom_profile,
            inheritance_chain,
            test_groups: &self.inner.test_groups,
            scripts: &self.inner.scripts,
            compiled_data,
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
            .map_err(|error| {
                // Both serde_path_to_error and the latest versions of the
                // config crate report the key. We drop the key from the config
                // error for consistency.
                let path = error.path().clone();
                let config_error = error.into_inner();
                let error = match config_error {
                    ConfigError::At { error, .. } => *error,
                    other => other,
                };
                ConfigParseErrorKind::DeserializeError(Box::new(serde_path_to_error::Error::new(
                    path, error,
                )))
            })?;

        Ok((config, ignored))
    }
}

/// The state of nextest profiles before build platforms have been applied.
#[derive(Clone, Debug, Default)]
pub(in crate::config) struct PreBuildPlatform {}

/// The state of nextest profiles after build platforms have been applied.
#[derive(Clone, Debug)]
pub(crate) struct FinalConfig {
    // Evaluation result for host_spec on the host platform.
    pub(in crate::config) host_eval: bool,
    // Evaluation result for target_spec corresponding to tests that run on the host platform (e.g.
    // proc-macro tests).
    pub(in crate::config) host_test_eval: bool,
    // Evaluation result for target_spec corresponding to tests that run on the target platform
    // (most regular tests).
    pub(in crate::config) target_eval: bool,
}

/// A nextest profile that can be obtained without identifying the host and
/// target platforms.
///
/// Returned by [`NextestConfig::profile`].
pub struct EarlyProfile<'cfg> {
    name: String,
    store_dir: Utf8PathBuf,
    default_profile: &'cfg DefaultProfileImpl,
    custom_profile: Option<&'cfg CustomProfileImpl>,
    inheritance_chain: Vec<&'cfg CustomProfileImpl>,
    test_groups: &'cfg BTreeMap<CustomTestGroup, TestGroupConfig>,
    // This is ordered because the scripts are used in the order they're defined.
    scripts: &'cfg ScriptConfig,
    // Invariant: `compiled_data.default_filter` is always present.
    pub(in crate::config) compiled_data: CompiledData<PreBuildPlatform>,
}

impl<'cfg> EarlyProfile<'cfg> {
    /// Returns the absolute profile-specific store directory.
    pub fn store_dir(&self) -> &Utf8Path {
        &self.store_dir
    }

    /// Returns the global test group configuration.
    pub fn test_group_config(&self) -> &'cfg BTreeMap<CustomTestGroup, TestGroupConfig> {
        self.test_groups
    }

    /// Applies build platforms to make the profile ready for evaluation.
    ///
    /// This is a separate step from parsing the config and reading a profile so that cargo-nextest
    /// can tell users about configuration parsing errors before building the binary list.
    pub fn apply_build_platforms(
        self,
        build_platforms: &BuildPlatforms,
    ) -> EvaluatableProfile<'cfg> {
        let compiled_data = self.compiled_data.apply_build_platforms(build_platforms);

        let resolved_default_filter = {
            // Look for the default filter in the first valid override.
            let found_filter = compiled_data
                .overrides
                .iter()
                .find_map(|override_data| override_data.default_filter_if_matches_platform());
            found_filter.unwrap_or_else(|| {
                // No overrides matching the default filter were found -- use
                // the profile's default.
                compiled_data
                    .profile_default_filter
                    .as_ref()
                    .expect("compiled data always has default set")
            })
        }
        .clone();

        EvaluatableProfile {
            name: self.name,
            store_dir: self.store_dir,
            default_profile: self.default_profile,
            custom_profile: self.custom_profile,
            inheritance_chain: self.inheritance_chain,
            scripts: self.scripts,
            test_groups: self.test_groups,
            compiled_data,
            resolved_default_filter,
        }
    }
}

/// A configuration profile for nextest. Contains most configuration used by the nextest runner.
///
/// Returned by [`EarlyProfile::apply_build_platforms`].
#[derive(Clone, Debug)]
pub struct EvaluatableProfile<'cfg> {
    name: String,
    store_dir: Utf8PathBuf,
    default_profile: &'cfg DefaultProfileImpl,
    custom_profile: Option<&'cfg CustomProfileImpl>,
    inheritance_chain: Vec<&'cfg CustomProfileImpl>,
    test_groups: &'cfg BTreeMap<CustomTestGroup, TestGroupConfig>,
    // This is ordered because the scripts are used in the order they're defined.
    scripts: &'cfg ScriptConfig,
    // Invariant: `compiled_data.default_filter` is always present.
    pub(in crate::config) compiled_data: CompiledData<FinalConfig>,
    // The default filter that's been resolved after considering overrides (i.e.
    // platforms).
    resolved_default_filter: CompiledDefaultFilter,
}

/// These macros return a specific config field from an EvaluatableProfile,
/// checking in order: custom profile, inheritance chain, then default profile.
macro_rules! profile_field {
    ($eval_prof:ident.$field:ident) => {
        $eval_prof
            .custom_profile
            .iter()
            .chain($eval_prof.inheritance_chain.iter())
            .find_map(|p| p.$field)
            .unwrap_or($eval_prof.default_profile.$field)
    };
    ($eval_prof:ident.$nested:ident.$field:ident) => {
        $eval_prof
            .custom_profile
            .iter()
            .chain($eval_prof.inheritance_chain.iter())
            .find_map(|p| p.$nested.$field)
            .unwrap_or($eval_prof.default_profile.$nested.$field)
    };
    // Variant for method calls with arguments.
    ($eval_prof:ident.$method:ident($($arg:expr),*)) => {
        $eval_prof
            .custom_profile
            .iter()
            .chain($eval_prof.inheritance_chain.iter())
            .find_map(|p| p.$method($($arg),*))
            .unwrap_or_else(|| $eval_prof.default_profile.$method($($arg),*))
    };
}
macro_rules! profile_field_from_ref {
    ($eval_prof:ident.$field:ident.$ref_func:ident()) => {
        $eval_prof
            .custom_profile
            .iter()
            .chain($eval_prof.inheritance_chain.iter())
            .find_map(|p| p.$field.$ref_func())
            .unwrap_or(&$eval_prof.default_profile.$field)
    };
    ($eval_prof:ident.$nested:ident.$field:ident.$ref_func:ident()) => {
        $eval_prof
            .custom_profile
            .iter()
            .chain($eval_prof.inheritance_chain.iter())
            .find_map(|p| p.$nested.$field.$ref_func())
            .unwrap_or(&$eval_prof.default_profile.$nested.$field)
    };
}
// Variant for fields where both custom and default are Option.
macro_rules! profile_field_optional {
    ($eval_prof:ident.$nested:ident.$field:ident.$ref_func:ident()) => {
        $eval_prof
            .custom_profile
            .iter()
            .chain($eval_prof.inheritance_chain.iter())
            .find_map(|p| p.$nested.$field.$ref_func())
            .or($eval_prof.default_profile.$nested.$field.$ref_func())
    };
}

impl<'cfg> EvaluatableProfile<'cfg> {
    /// Returns the name of the profile.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the absolute profile-specific store directory.
    pub fn store_dir(&self) -> &Utf8Path {
        &self.store_dir
    }

    /// Returns the context in which to evaluate filtersets.
    pub fn filterset_ecx(&self) -> EvalContext<'_> {
        EvalContext {
            default_filter: &self.default_filter().expr,
        }
    }

    /// Returns the default set of tests to run.
    pub fn default_filter(&self) -> &CompiledDefaultFilter {
        &self.resolved_default_filter
    }

    /// Returns the global test group configuration.
    pub fn test_group_config(&self) -> &'cfg BTreeMap<CustomTestGroup, TestGroupConfig> {
        self.test_groups
    }

    /// Returns the global script configuration.
    pub fn script_config(&self) -> &'cfg ScriptConfig {
        self.scripts
    }

    /// Returns the retry count for this profile.
    pub fn retries(&self) -> RetryPolicy {
        profile_field!(self.retries)
    }

    /// Returns the number of threads to run against for this profile.
    pub fn test_threads(&self) -> TestThreads {
        profile_field!(self.test_threads)
    }

    /// Returns the number of threads required for each test.
    pub fn threads_required(&self) -> ThreadsRequired {
        profile_field!(self.threads_required)
    }

    /// Returns extra arguments to be passed to the test binary at runtime.
    pub fn run_extra_args(&self) -> &'cfg [String] {
        profile_field_from_ref!(self.run_extra_args.as_deref())
    }

    /// Returns the time after which tests are treated as slow for this profile.
    pub fn slow_timeout(&self, run_mode: NextestRunMode) -> SlowTimeout {
        profile_field!(self.slow_timeout(run_mode))
    }

    /// Returns the time after which we should stop running tests.
    pub fn global_timeout(&self, run_mode: NextestRunMode) -> GlobalTimeout {
        profile_field!(self.global_timeout(run_mode))
    }

    /// Returns the time after which a child process that hasn't closed its handles is marked as
    /// leaky.
    pub fn leak_timeout(&self) -> LeakTimeout {
        profile_field!(self.leak_timeout)
    }

    /// Returns the test status level.
    pub fn status_level(&self) -> StatusLevel {
        profile_field!(self.status_level)
    }

    /// Returns the test status level at the end of the run.
    pub fn final_status_level(&self) -> FinalStatusLevel {
        profile_field!(self.final_status_level)
    }

    /// Returns the failure output config for this profile.
    pub fn failure_output(&self) -> TestOutputDisplay {
        profile_field!(self.failure_output)
    }

    /// Returns the failure output config for this profile.
    pub fn success_output(&self) -> TestOutputDisplay {
        profile_field!(self.success_output)
    }

    /// Returns the max-fail config for this profile.
    pub fn max_fail(&self) -> MaxFail {
        profile_field!(self.max_fail)
    }

    /// Returns the archive configuration for this profile.
    pub fn archive_config(&self) -> &'cfg ArchiveConfig {
        profile_field_from_ref!(self.archive.as_ref())
    }

    /// Returns the list of setup scripts.
    pub fn setup_scripts(&self, test_list: &TestList<'_>) -> SetupScripts<'_> {
        SetupScripts::new(self, test_list)
    }

    /// Returns list-time settings for a test binary.
    pub fn list_settings_for(&self, query: &BinaryQuery<'_>) -> ListSettings<'_> {
        ListSettings::new(self, query)
    }

    /// Returns settings for individual tests.
    pub fn settings_for(
        &self,
        run_mode: NextestRunMode,
        query: &TestQuery<'_>,
    ) -> TestSettings<'_> {
        TestSettings::new(self, run_mode, query)
    }

    /// Returns override settings for individual tests, with sources attached.
    pub(crate) fn settings_with_source_for(
        &self,
        run_mode: NextestRunMode,
        query: &TestQuery<'_>,
    ) -> TestSettings<'_, SettingSource<'_>> {
        TestSettings::new(self, run_mode, query)
    }

    /// Returns the JUnit configuration for this profile.
    pub fn junit(&self) -> Option<JunitConfig<'cfg>> {
        let settings = JunitSettings {
            path: profile_field_optional!(self.junit.path.as_deref()),
            report_name: profile_field_from_ref!(self.junit.report_name.as_deref()),
            store_success_output: profile_field!(self.junit.store_success_output),
            store_failure_output: profile_field!(self.junit.store_failure_output),
        };
        JunitConfig::new(self.store_dir(), settings)
    }

    /// Returns the profile that this profile inherits from.
    pub fn inherits(&self) -> Option<&str> {
        if let Some(custom_profile) = self.custom_profile {
            return custom_profile.inherits();
        }
        None
    }

    #[cfg(test)]
    pub(in crate::config) fn custom_profile(&self) -> Option<&'cfg CustomProfileImpl> {
        self.custom_profile
    }
}

#[derive(Clone, Debug)]
pub(in crate::config) struct NextestConfigImpl {
    store: StoreConfigImpl,
    test_groups: BTreeMap<CustomTestGroup, TestGroupConfig>,
    scripts: ScriptConfig,
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

    pub(in crate::config) fn default_profile(&self) -> &DefaultProfileImpl {
        &self.default_profile
    }

    pub(in crate::config) fn other_profiles(
        &self,
    ) -> impl Iterator<Item = (&str, &CustomProfileImpl)> {
        self.other_profiles
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }

    /// Resolve a profile's inheritance chain (ancestors only, not including the
    /// profile itself).
    ///
    /// Returns the chain ordered from immediate parent to furthest ancestor.
    /// Cycles are assumed to have been checked by `sanitize_profile_inherits()`.
    fn resolve_inheritance_chain(
        &self,
        profile_name: &str,
    ) -> Result<Vec<&CustomProfileImpl>, ProfileNotFound> {
        let mut chain = Vec::new();

        // Start from the profile's parent, not the profile itself (the profile
        // is already available via custom_profile).
        let mut curr = self
            .get_profile(profile_name)?
            .and_then(|p| p.inherits.as_deref());

        while let Some(name) = curr {
            let profile = self.get_profile(name)?;
            if let Some(profile) = profile {
                chain.push(profile);
                curr = profile.inherits.as_deref();
            } else {
                // Reached the default profile -- stop.
                break;
            }
        }

        Ok(chain)
    }

    /// Sanitize inherits settings on default and custom profiles.
    ///
    /// `known_profiles` contains profiles from previously loaded (lower priority) files.
    /// A profile can inherit from profiles in the same file or in `known_profiles`.
    fn sanitize_profile_inherits(
        &self,
        known_profiles: &BTreeSet<String>,
    ) -> Result<(), ConfigParseErrorKind> {
        let mut inherit_err_collector = Vec::new();

        self.sanitize_default_profile_inherits(&mut inherit_err_collector);
        self.sanitize_custom_profile_inherits(&mut inherit_err_collector, known_profiles);

        if !inherit_err_collector.is_empty() {
            return Err(ConfigParseErrorKind::InheritanceErrors(
                inherit_err_collector,
            ));
        }

        Ok(())
    }

    /// Check the DefaultProfileImpl and make sure that it doesn't inherit from other
    /// profiles
    fn sanitize_default_profile_inherits(&self, inherit_err_collector: &mut Vec<InheritsError>) {
        if self.default_profile().inherits().is_some() {
            inherit_err_collector.push(InheritsError::DefaultProfileInheritance(
                NextestConfig::DEFAULT_PROFILE.to_string(),
            ));
        }
    }

    /// Iterate through each custom profile inherits and report any inheritance error(s).
    fn sanitize_custom_profile_inherits(
        &self,
        inherit_err_collector: &mut Vec<InheritsError>,
        known_profiles: &BTreeSet<String>,
    ) {
        let mut profile_graph = Graph::<&str, (), Directed>::new();
        let mut profile_map = HashMap::new();

        // Iterate through all custom profiles within the config file and constructs
        // a reduced graph of the inheritance chain(s)
        for (name, custom_profile) in self.other_profiles() {
            let starts_with_default = self.sanitize_custom_default_profile_inherits(
                name,
                custom_profile,
                inherit_err_collector,
            );
            if !starts_with_default {
                // We don't need to add default- profiles. Since they cannot
                // have inherits specified on them (they effectively always
                // inherit from default), they cannot participate in inheritance
                // cycles.
                self.add_profile_to_graph(
                    name,
                    custom_profile,
                    &mut profile_map,
                    &mut profile_graph,
                    inherit_err_collector,
                    known_profiles,
                );
            }
        }

        self.check_inheritance_cycles(profile_graph, inherit_err_collector);
    }

    /// Check any CustomProfileImpl that have a "default-" name and make sure they
    /// do not inherit from other profiles.
    fn sanitize_custom_default_profile_inherits(
        &self,
        name: &str,
        custom_profile: &CustomProfileImpl,
        inherit_err_collector: &mut Vec<InheritsError>,
    ) -> bool {
        let starts_with_default = name.starts_with("default-");

        if starts_with_default && custom_profile.inherits().is_some() {
            inherit_err_collector.push(InheritsError::DefaultProfileInheritance(name.to_string()));
        }

        starts_with_default
    }

    /// Add the custom profile to the profile graph and collect any inheritance errors like
    /// self-referential profiles and nonexisting profiles.
    ///
    /// `known_profiles` contains profiles from previously loaded (lower priority) files.
    fn add_profile_to_graph<'cfg>(
        &self,
        name: &'cfg str,
        custom_profile: &'cfg CustomProfileImpl,
        profile_map: &mut HashMap<&'cfg str, NodeIndex>,
        profile_graph: &mut Graph<&'cfg str, ()>,
        inherit_err_collector: &mut Vec<InheritsError>,
        known_profiles: &BTreeSet<String>,
    ) {
        if let Some(inherits_name) = custom_profile.inherits() {
            if inherits_name == name {
                inherit_err_collector
                    .push(InheritsError::SelfReferentialInheritance(name.to_string()))
            } else if self.get_profile(inherits_name).is_ok() {
                // Inherited profile exists in this file -- create edge for cycle detection.
                let from_node = match profile_map.get(name) {
                    None => {
                        let profile_node = profile_graph.add_node(name);
                        profile_map.insert(name, profile_node);
                        profile_node
                    }
                    Some(node_idx) => *node_idx,
                };
                let to_node = match profile_map.get(inherits_name) {
                    None => {
                        let profile_node = profile_graph.add_node(inherits_name);
                        profile_map.insert(inherits_name, profile_node);
                        profile_node
                    }
                    Some(node_idx) => *node_idx,
                };
                profile_graph.add_edge(from_node, to_node, ());
            } else if known_profiles.contains(inherits_name) {
                // Inherited profile exists in a previously loaded file -- valid, no
                // cycle detection needed (cross-file cycles are impossible with
                // downward-only inheritance).
            } else {
                inherit_err_collector.push(InheritsError::UnknownInheritance(
                    name.to_string(),
                    inherits_name.to_string(),
                ))
            }
        }
    }

    /// Given a profile graph, reports all SCC cycles within the graph using kosaraju algorithm.
    fn check_inheritance_cycles(
        &self,
        profile_graph: Graph<&str, ()>,
        inherit_err_collector: &mut Vec<InheritsError>,
    ) {
        let profile_sccs: Vec<Vec<NodeIndex>> = kosaraju_scc(&profile_graph);
        let profile_sccs: Vec<Vec<NodeIndex>> = profile_sccs
            .into_iter()
            .filter(|scc| scc.len() >= 2)
            .collect();

        if !profile_sccs.is_empty() {
            inherit_err_collector.push(InheritsError::InheritanceCycle(
                profile_sccs
                    .iter()
                    .map(|node_idxs| {
                        let profile_names: Vec<String> = node_idxs
                            .iter()
                            .map(|node_idx| profile_graph[*node_idx].to_string())
                            .collect();
                        profile_names
                    })
                    .collect(),
            ));
        }
    }
}

// This is the form of `NextestConfig` that gets deserialized.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct NextestConfigDeserialize {
    store: StoreConfigImpl,

    // These are parsed as part of NextestConfigVersionOnly. They're re-parsed here to avoid
    // printing an "unknown key" message.
    #[expect(unused)]
    #[serde(default)]
    nextest_version: Option<NextestVersionDeserialize>,
    #[expect(unused)]
    #[serde(default)]
    experimental: ExperimentalDeserialize,

    #[serde(default)]
    test_groups: BTreeMap<CustomTestGroup, TestGroupConfig>,
    // Previous version of setup scripts, stored as "script.<name of script>".
    #[serde(default, rename = "script")]
    old_setup_scripts: IndexMap<ScriptId, SetupScriptConfig>,
    #[serde(default)]
    scripts: ScriptConfig,
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

        // XXX: This is not quite right (doesn't obey precedence) but is okay
        // because it's unlikely folks are using the combination of setup
        // scripts *and* tools *and* relying on this. If it breaks, well, this
        // feature isn't stable.
        for (script_id, script_config) in self.old_setup_scripts {
            if let indexmap::map::Entry::Vacant(entry) = self.scripts.setup.entry(script_id) {
                entry.insert(script_config);
            }
        }

        NextestConfigImpl {
            store: self.store,
            default_profile,
            test_groups: self.test_groups,
            scripts: self.scripts,
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
pub(in crate::config) struct DefaultProfileImpl {
    default_filter: String,
    test_threads: TestThreads,
    threads_required: ThreadsRequired,
    run_extra_args: Vec<String>,
    retries: RetryPolicy,
    status_level: StatusLevel,
    final_status_level: FinalStatusLevel,
    failure_output: TestOutputDisplay,
    success_output: TestOutputDisplay,
    max_fail: MaxFail,
    slow_timeout: SlowTimeout,
    global_timeout: GlobalTimeout,
    leak_timeout: LeakTimeout,
    overrides: Vec<DeserializedOverride>,
    scripts: Vec<DeserializedProfileScriptConfig>,
    junit: DefaultJunitImpl,
    archive: ArchiveConfig,
    bench: DefaultBenchConfig,
    inherits: Inherits,
}

impl DefaultProfileImpl {
    fn new(p: CustomProfileImpl) -> Self {
        Self {
            default_filter: p
                .default_filter
                .expect("default-filter present in default profile"),
            test_threads: p
                .test_threads
                .expect("test-threads present in default profile"),
            threads_required: p
                .threads_required
                .expect("threads-required present in default profile"),
            run_extra_args: p
                .run_extra_args
                .expect("run-extra-args present in default profile"),
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
            max_fail: p.max_fail.expect("fail-fast present in default profile"),
            slow_timeout: p
                .slow_timeout
                .expect("slow-timeout present in default profile"),
            global_timeout: p
                .global_timeout
                .expect("global-timeout present in default profile"),
            leak_timeout: p
                .leak_timeout
                .expect("leak-timeout present in default profile"),
            overrides: p.overrides,
            scripts: p.scripts,
            junit: DefaultJunitImpl::for_default_profile(p.junit),
            archive: p.archive.expect("archive present in default profile"),
            bench: DefaultBenchConfig::for_default_profile(
                p.bench.expect("bench present in default profile"),
            ),
            inherits: Inherits::new(p.inherits),
        }
    }

    pub(in crate::config) fn default_filter(&self) -> &str {
        &self.default_filter
    }

    pub(in crate::config) fn inherits(&self) -> Option<&str> {
        self.inherits.inherits_from()
    }

    pub(in crate::config) fn overrides(&self) -> &[DeserializedOverride] {
        &self.overrides
    }

    pub(in crate::config) fn setup_scripts(&self) -> &[DeserializedProfileScriptConfig] {
        &self.scripts
    }

    pub(in crate::config) fn slow_timeout(&self, run_mode: NextestRunMode) -> SlowTimeout {
        match run_mode {
            NextestRunMode::Test => self.slow_timeout,
            NextestRunMode::Benchmark => self.bench.slow_timeout,
        }
    }

    pub(in crate::config) fn global_timeout(&self, run_mode: NextestRunMode) -> GlobalTimeout {
        match run_mode {
            NextestRunMode::Test => self.global_timeout,
            NextestRunMode::Benchmark => self.bench.global_timeout,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::config) struct CustomProfileImpl {
    /// The default set of tests run by `cargo nextest run`.
    #[serde(default)]
    default_filter: Option<String>,
    #[serde(default, deserialize_with = "deserialize_retry_policy")]
    retries: Option<RetryPolicy>,
    #[serde(default)]
    test_threads: Option<TestThreads>,
    #[serde(default)]
    threads_required: Option<ThreadsRequired>,
    #[serde(default)]
    run_extra_args: Option<Vec<String>>,
    #[serde(default)]
    status_level: Option<StatusLevel>,
    #[serde(default)]
    final_status_level: Option<FinalStatusLevel>,
    #[serde(default)]
    failure_output: Option<TestOutputDisplay>,
    #[serde(default)]
    success_output: Option<TestOutputDisplay>,
    #[serde(
        default,
        rename = "fail-fast",
        deserialize_with = "deserialize_fail_fast"
    )]
    max_fail: Option<MaxFail>,
    #[serde(default, deserialize_with = "deserialize_slow_timeout")]
    slow_timeout: Option<SlowTimeout>,
    #[serde(default)]
    global_timeout: Option<GlobalTimeout>,
    #[serde(default, deserialize_with = "deserialize_leak_timeout")]
    leak_timeout: Option<LeakTimeout>,
    #[serde(default)]
    overrides: Vec<DeserializedOverride>,
    #[serde(default)]
    scripts: Vec<DeserializedProfileScriptConfig>,
    #[serde(default)]
    junit: JunitImpl,
    #[serde(default)]
    archive: Option<ArchiveConfig>,
    #[serde(default)]
    bench: Option<BenchConfig>,
    #[serde(default)]
    inherits: Option<String>,
}

impl CustomProfileImpl {
    #[cfg(test)]
    pub(in crate::config) fn test_threads(&self) -> Option<TestThreads> {
        self.test_threads
    }

    pub(in crate::config) fn default_filter(&self) -> Option<&str> {
        self.default_filter.as_deref()
    }

    pub(in crate::config) fn slow_timeout(&self, run_mode: NextestRunMode) -> Option<SlowTimeout> {
        match run_mode {
            NextestRunMode::Test => self.slow_timeout,
            NextestRunMode::Benchmark => self.bench.as_ref().and_then(|b| b.slow_timeout),
        }
    }

    pub(in crate::config) fn global_timeout(
        &self,
        run_mode: NextestRunMode,
    ) -> Option<GlobalTimeout> {
        match run_mode {
            NextestRunMode::Test => self.global_timeout,
            NextestRunMode::Benchmark => self.bench.as_ref().and_then(|b| b.global_timeout),
        }
    }

    pub(in crate::config) fn inherits(&self) -> Option<&str> {
        self.inherits.as_deref()
    }

    pub(in crate::config) fn overrides(&self) -> &[DeserializedOverride] {
        &self.overrides
    }

    pub(in crate::config) fn scripts(&self) -> &[DeserializedProfileScriptConfig] {
        &self.scripts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::utils::test_helpers::*;
    use camino_tempfile::tempdir;
    use iddqd::{IdHashItem, IdHashMap, id_hash_map, id_upcast};

    fn tool_name(s: &str) -> ToolName {
        ToolName::new(s.into()).unwrap()
    }

    /// Test implementation of ConfigWarnings that collects warnings for testing.
    #[derive(Default)]
    struct TestConfigWarnings {
        unknown_keys: IdHashMap<UnknownKeys>,
        reserved_profiles: IdHashMap<ReservedProfiles>,
        deprecated_scripts: IdHashMap<DeprecatedScripts>,
        empty_script_warnings: IdHashMap<EmptyScriptSections>,
    }

    impl ConfigWarnings for TestConfigWarnings {
        fn unknown_config_keys(
            &mut self,
            config_file: &Utf8Path,
            _workspace_root: &Utf8Path,
            tool: Option<&ToolName>,
            unknown: &BTreeSet<String>,
        ) {
            self.unknown_keys
                .insert_unique(UnknownKeys {
                    tool: tool.cloned(),
                    config_file: config_file.to_owned(),
                    keys: unknown.clone(),
                })
                .unwrap();
        }

        fn unknown_reserved_profiles(
            &mut self,
            config_file: &Utf8Path,
            _workspace_root: &Utf8Path,
            tool: Option<&ToolName>,
            profiles: &[&str],
        ) {
            self.reserved_profiles
                .insert_unique(ReservedProfiles {
                    tool: tool.cloned(),
                    config_file: config_file.to_owned(),
                    profiles: profiles.iter().map(|&s| s.to_owned()).collect(),
                })
                .unwrap();
        }

        fn empty_script_sections(
            &mut self,
            config_file: &Utf8Path,
            _workspace_root: &Utf8Path,
            tool: Option<&ToolName>,
            profile_name: &str,
            empty_count: usize,
        ) {
            self.empty_script_warnings
                .insert_unique(EmptyScriptSections {
                    tool: tool.cloned(),
                    config_file: config_file.to_owned(),
                    profile_name: profile_name.to_owned(),
                    empty_count,
                })
                .unwrap();
        }

        fn deprecated_script_config(
            &mut self,
            config_file: &Utf8Path,
            _workspace_root: &Utf8Path,
            tool: Option<&ToolName>,
        ) {
            self.deprecated_scripts
                .insert_unique(DeprecatedScripts {
                    tool: tool.cloned(),
                    config_file: config_file.to_owned(),
                })
                .unwrap();
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct UnknownKeys {
        tool: Option<ToolName>,
        config_file: Utf8PathBuf,
        keys: BTreeSet<String>,
    }

    impl IdHashItem for UnknownKeys {
        type Key<'a> = Option<&'a ToolName>;
        fn key(&self) -> Self::Key<'_> {
            self.tool.as_ref()
        }
        id_upcast!();
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct ReservedProfiles {
        tool: Option<ToolName>,
        config_file: Utf8PathBuf,
        profiles: Vec<String>,
    }

    impl IdHashItem for ReservedProfiles {
        type Key<'a> = Option<&'a ToolName>;
        fn key(&self) -> Self::Key<'_> {
            self.tool.as_ref()
        }
        id_upcast!();
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct DeprecatedScripts {
        tool: Option<ToolName>,
        config_file: Utf8PathBuf,
    }

    impl IdHashItem for DeprecatedScripts {
        type Key<'a> = Option<&'a ToolName>;
        fn key(&self) -> Self::Key<'_> {
            self.tool.as_ref()
        }
        id_upcast!();
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct EmptyScriptSections {
        tool: Option<ToolName>,
        config_file: Utf8PathBuf,
        profile_name: String,
        empty_count: usize,
    }

    impl IdHashItem for EmptyScriptSections {
        type Key<'a> = (&'a Option<ToolName>, &'a str);
        fn key(&self) -> Self::Key<'_> {
            (&self.tool, &self.profile_name)
        }
        id_upcast!();
    }

    #[test]
    fn default_config_is_valid() {
        let default_config = NextestConfig::default_config("foo");
        default_config
            .profile(NextestConfig::DEFAULT_PROFILE)
            .expect("default profile should exist");
    }

    #[test]
    fn ignored_keys() {
        let config_contents = r#"
        ignored1 = "test"

        [profile.default]
        retries = 3
        ignored2 = "hi"

        [profile.default-foo]
        retries = 5

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

        [profile.default-bar]
        retries = 5

        [profile.tool]
        retries = 12

        [[profile.tool.overrides]]
        filter = 'test(test_baz)'
        retries = 22
        ignored6 = 6.5
        "#;

        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);
        let workspace_root = graph.workspace().root();
        let tool_path = workspace_root.join(".config/tool.toml");
        std::fs::write(&tool_path, tool_config_contents).unwrap();

        let pcx = ParseContext::new(&graph);

        let mut warnings = TestConfigWarnings::default();

        let _ = NextestConfig::from_sources_with_warnings(
            workspace_root,
            &pcx,
            None,
            &[ToolConfigFile {
                tool: tool_name("my-tool"),
                config_file: tool_path.clone(),
            }][..],
            &Default::default(),
            &mut warnings,
        )
        .expect("config is valid");

        assert_eq!(
            warnings.unknown_keys.len(),
            2,
            "there are two files with unknown keys"
        );

        assert_eq!(
            warnings.unknown_keys,
            id_hash_map! {
                UnknownKeys {
                    tool: None,
                    config_file: workspace_root.join(".config/nextest.toml"),
                    keys: maplit::btreeset! {
                        "ignored1".to_owned(),
                        "profile.default.ignored2".to_owned(),
                        "profile.default.overrides.0.ignored3".to_owned(),
                    }
                },
                UnknownKeys {
                    tool: Some(tool_name("my-tool")),
                    config_file: tool_path.clone(),
                    keys: maplit::btreeset! {
                        "store.ignored4".to_owned(),
                        "profile.default.ignored5".to_owned(),
                        "profile.tool.overrides.0.ignored6".to_owned(),
                    }
                }
            }
        );
        assert_eq!(
            warnings.reserved_profiles,
            id_hash_map! {
                ReservedProfiles {
                    tool: None,
                    config_file: workspace_root.join(".config/nextest.toml"),
                    profiles: vec!["default-foo".to_owned()],
                },
                ReservedProfiles {
                    tool: Some(tool_name("my-tool")),
                    config_file: tool_path,
                    profiles: vec!["default-bar".to_owned()],
                }
            },
        )
    }

    #[test]
    fn script_warnings() {
        let config_contents = r#"
        experimental = ["setup-scripts", "wrapper-scripts"]

        [scripts.wrapper.script1]
        command = "echo test"

        [scripts.wrapper.script2]
        command = "echo test2"

        [scripts.setup.script3]
        command = "echo setup"

        [[profile.default.scripts]]
        filter = 'all()'
        # Empty - no setup or wrapper scripts

        [[profile.default.scripts]]
        filter = 'test(foo)'
        setup = ["script3"]

        [profile.custom]
        [[profile.custom.scripts]]
        filter = 'all()'
        # Empty - no setup or wrapper scripts

        [[profile.custom.scripts]]
        filter = 'test(bar)'
        # Another empty section
        "#;

        let tool_config_contents = r#"
        experimental = ["setup-scripts", "wrapper-scripts"]

        [scripts.wrapper."@tool:tool:disabled_script"]
        command = "echo disabled"

        [scripts.setup."@tool:tool:setup_script"]
        command = "echo setup"

        [profile.tool]
        [[profile.tool.scripts]]
        filter = 'all()'
        # Empty section

        [[profile.tool.scripts]]
        filter = 'test(foo)'
        setup = ["@tool:tool:setup_script"]
        "#;

        let workspace_dir = tempdir().unwrap();
        let graph = temp_workspace(&workspace_dir, config_contents);
        let workspace_root = graph.workspace().root();
        let tool_path = workspace_root.join(".config/tool.toml");
        std::fs::write(&tool_path, tool_config_contents).unwrap();

        let pcx = ParseContext::new(&graph);

        let mut warnings = TestConfigWarnings::default();

        let experimental = maplit::btreeset! {
            ConfigExperimental::SetupScripts,
            ConfigExperimental::WrapperScripts
        };
        let _ = NextestConfig::from_sources_with_warnings(
            workspace_root,
            &pcx,
            None,
            &[ToolConfigFile {
                tool: tool_name("tool"),
                config_file: tool_path.clone(),
            }][..],
            &experimental,
            &mut warnings,
        )
        .expect("config is valid");

        assert_eq!(
            warnings.empty_script_warnings,
            id_hash_map! {
                EmptyScriptSections {
                    tool: None,
                    config_file: workspace_root.join(".config/nextest.toml"),
                    profile_name: "default".to_owned(),
                    empty_count: 1,
                },
                EmptyScriptSections {
                    tool: None,
                    config_file: workspace_root.join(".config/nextest.toml"),
                    profile_name: "custom".to_owned(),
                    empty_count: 2,
                },
                EmptyScriptSections {
                    tool: Some(tool_name("tool")),
                    config_file: tool_path,
                    profile_name: "tool".to_owned(),
                    empty_count: 1,
                }
            }
        );
    }

    #[test]
    fn deprecated_script_config_warning() {
        let config_contents = r#"
        experimental = ["setup-scripts"]

        [script.my-script]
        command = "echo hello"
"#;

        let tool_config_contents = r#"
        experimental = ["setup-scripts"]

        [script."@tool:my-tool:my-script"]
        command = "echo hello"
"#;

        let temp_dir = tempdir().unwrap();

        let graph = temp_workspace(&temp_dir, config_contents);
        let workspace_root = graph.workspace().root();
        let tool_path = workspace_root.join(".config/my-tool.toml");
        std::fs::write(&tool_path, tool_config_contents).unwrap();
        let pcx = ParseContext::new(&graph);

        let mut warnings = TestConfigWarnings::default();
        NextestConfig::from_sources_with_warnings(
            graph.workspace().root(),
            &pcx,
            None,
            &[ToolConfigFile {
                tool: tool_name("my-tool"),
                config_file: tool_path.clone(),
            }],
            &maplit::btreeset! {ConfigExperimental::SetupScripts},
            &mut warnings,
        )
        .expect("config is valid");

        assert_eq!(
            warnings.deprecated_scripts,
            id_hash_map! {
                DeprecatedScripts {
                    tool: None,
                    config_file: graph.workspace().root().join(".config/nextest.toml"),
                },
                DeprecatedScripts {
                    tool: Some(tool_name("my-tool")),
                    config_file: tool_path,
                }
            }
        );
    }
}
