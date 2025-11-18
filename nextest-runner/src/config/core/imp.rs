// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{NextestVersionDeserialize, ToolConfigFile};
use crate::{
    config::{
        core::ConfigExperimental,
        elements::{
            ArchiveConfig, CustomTestGroup, DefaultJunitImpl, GlobalTimeout, JunitConfig,
            JunitImpl, LeakTimeout, MaxFail, RetryPolicy, SlowTimeout, TestGroup, TestGroupConfig,
            TestThreads, ThreadsRequired, deserialize_fail_fast, deserialize_leak_timeout,
            deserialize_retry_policy, deserialize_slow_timeout,
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
        ConfigParseError, ConfigParseErrorKind, ProfileListScriptUsesRunFiltersError,
        ProfileNotFound, ProfileScriptErrors, ProfileUnknownScriptError,
        ProfileWrongConfigScriptTypeError, UnknownTestGroupError, provided_by_tool,
    },
    helpers::plural,
    list::TestList,
    platform::BuildPlatforms,
    reporter::{FinalStatusLevel, StatusLevel, TestOutputDisplay},
};
use camino::{Utf8Path, Utf8PathBuf};
use config::{
    Config, ConfigBuilder, ConfigError, File, FileFormat, FileSourceFile, builder::DefaultState,
};
use iddqd::IdOrdMap;
use indexmap::IndexMap;
use nextest_filtering::{BinaryQuery, EvalContext, Filterset, ParseContext, TestQuery};
use petgraph::{Directed, Graph, algo::scc::kosaraju_scc};
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
        tool: Option<&str>,
        unknown: &BTreeSet<String>,
    );

    /// Handle unknown profiles found in the reserved `default-` namespace.
    fn unknown_reserved_profiles(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&str>,
        profiles: &[&str],
    );

    /// Handle deprecated `[script.*]` configuration.
    fn deprecated_script_config(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&str>,
    );

    /// Handle warning about empty script sections with neither setup nor
    /// wrapper scripts.
    fn empty_script_sections(
        &mut self,
        config_file: &Utf8Path,
        workspace_root: &Utf8Path,
        tool: Option<&str>,
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
        tool: Option<&str>,
        unknown: &BTreeSet<String>,
    ) {
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

        warn!(
            "in config file {}{}, ignoring unknown configuration keys: {unknown_str}",
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
        tool: Option<&str>,
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
        tool: Option<&str>,
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
        tool: Option<&str>,
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
        )?;

        composite_builder = composite_builder.add_source(source);

        // The unknown set is ignored here because any values in it have already been reported in
        // deserialize_individual_config.
        let (config, _unknown) = Self::build_and_deserialize_config(&composite_builder)
            .map_err(|kind| ConfigParseError::new(config_file, None, kind))?;

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
        tool: Option<&str>,
        source: File<FileSourceFile, FileFormat>,
        compiled_out: &mut CompiledByProfile,
        experimental: &BTreeSet<ConfigExperimental>,
        warnings: &mut impl ConfigWarnings,
        known_groups: &mut BTreeSet<CustomTestGroup>,
        known_scripts: &mut IdOrdMap<ScriptInfo>,
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
                        .is_some_and(|(tool_name, _)| tool_name == tool)
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
                        .is_some_and(|(tool_name, _)| tool_name == tool)
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

        // Observe if the config file has a cycle in the inheritance chain
        this_config.check_inheritance_cycles()?;

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

        // Resolves the inherit setting into a profile chain
        let inheritance_chain = if let Some(_) = custom_profile {
            self.inner.resolve_profile_chain(name)?
        } else {
            Vec::new()
        };

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

// TODO: macros for profile_config_field with consideration
// of inheritance chain
macro_rules! profile_config_field {
    () => {};
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

    /// Returns extra arguments to be passed to the test binary at runtime.
    pub fn run_extra_args(&self) -> &'cfg [String] {
        self.custom_profile
            .and_then(|profile| profile.run_extra_args.as_deref())
            .unwrap_or(&self.default_profile.run_extra_args)
    }

    /// Returns the time after which tests are treated as slow for this profile.
    pub fn slow_timeout(&self) -> SlowTimeout {
        self.custom_profile
            .and_then(|profile| profile.slow_timeout)
            .unwrap_or(self.default_profile.slow_timeout)
    }

    /// Returns the time after which we should stop running tests.
    pub fn global_timeout(&self) -> GlobalTimeout {
        self.custom_profile
            .and_then(|profile| profile.global_timeout)
            .unwrap_or(self.default_profile.global_timeout)
    }

    /// Returns the time after which a child process that hasn't closed its handles is marked as
    /// leaky.
    pub fn leak_timeout(&self) -> LeakTimeout {
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

    /// Returns the max-fail config for this profile.
    pub fn max_fail(&self) -> MaxFail {
        self.custom_profile
            .and_then(|profile| profile.max_fail)
            .unwrap_or(self.default_profile.max_fail)
    }

    /// Returns the archive configuration for this profile.
    pub fn archive_config(&self) -> &'cfg ArchiveConfig {
        self.custom_profile
            .and_then(|profile| profile.archive.as_ref())
            .unwrap_or(&self.default_profile.archive)
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
    pub fn settings_for(&self, query: &TestQuery<'_>) -> TestSettings<'_> {
        TestSettings::new(self, query)
    }

    /// Returns override settings for individual tests, with sources attached.
    pub(crate) fn settings_with_source_for(
        &self,
        query: &TestQuery<'_>,
    ) -> TestSettings<'_, SettingSource<'_>> {
        TestSettings::new(self, query)
    }

    /// Returns the JUnit configuration for this profile.
    pub fn junit(&self) -> Option<JunitConfig<'cfg>> {
        JunitConfig::new(
            self.store_dir(),
            self.custom_profile.map(|p| &p.junit),
            &self.default_profile.junit,
        )
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

    /// Resolves a profile with an inheritance chain recursively
    ///
    /// This function does not check for cycles. Use `check_inheritance_cycles()`
    /// to observe for cycles in an inheritance chain.
    fn resolve_profile_chain(
        &self,
        profile_name: &str,
    ) -> Result<Vec<&CustomProfileImpl>, ProfileNotFound> {
        // let mut visited = HashSet::new();
        let mut chain = Vec::new();

        self.resolve_profile_chain_recursive(profile_name, &mut chain)?;
        Ok(chain)
    }

    /// Helper function for resolving an inheritance chain
    fn resolve_profile_chain_recursive<'cfg>(
        &'cfg self,
        profile_name: &str,
        chain: &mut Vec<&'cfg CustomProfileImpl>,
    ) -> Result<(), ProfileNotFound> {
        let profile = self.get_profile(profile_name)?;
        if let Some(profile) = profile {
            if let Some(parent_name) = &profile.inherits {
                self.resolve_profile_chain_recursive(&parent_name, chain)?;
            }
            chain.push(profile);
        }

        Ok(())
    }

    /// Checks if a cycle exists in an inheritance chain
    fn check_inheritance_cycles(&self) -> Result<(), ConfigParseError> {
        let mut profile_graph = Graph::<&str, (), Directed>::new();
        let mut profile_map = HashMap::new();

        // Grab all profile names and insert into map
        for profile in self.all_profiles() {
            let profile_node = profile_graph.add_node(profile);
            profile_map.insert(profile.to_string(), profile_node);
        }

        // For each custom profile, we add a directed edge from the inherited node
        // to the current custom profile node
        for (profile_name, profile) in &self.other_profiles {
            if let Some(inherit_name) = &profile.inherits {
                if let (Some(&from), Some(&to)) =
                    (profile_map.get(inherit_name), profile_map.get(profile_name))
                {
                    profile_graph.add_edge(from, to, ());
                }
            }
        }

        // Detects all strongly connected components (SCCs) within the graph
        // and if there are exists any (or multiple), returns an error with
        // all SCCs
        let profile_sccs = kosaraju_scc(&profile_graph);
        if profile_sccs.len() != 0 {
            return Err(ConfigParseError::new(
                "inheritance cycle detected in profile configuration",
                None,
                ConfigParseErrorKind::InheritanceCycle(
                    profile_sccs
                        .iter()
                        .map(|profile_scc| profile_graph[profile_scc[0]].to_string())
                        .collect(),
                ),
            ));
        }

        Ok(())
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
    experimental: BTreeSet<String>,

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
        }
    }

    pub(in crate::config) fn default_filter(&self) -> &str {
        &self.default_filter
    }

    pub(in crate::config) fn overrides(&self) -> &[DeserializedOverride] {
        &self.overrides
    }

    pub(in crate::config) fn setup_scripts(&self) -> &[DeserializedProfileScriptConfig] {
        &self.scripts
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
            tool: Option<&str>,
            unknown: &BTreeSet<String>,
        ) {
            self.unknown_keys
                .insert_unique(UnknownKeys {
                    tool: tool.map(|s| s.to_owned()),
                    config_file: config_file.to_owned(),
                    keys: unknown.clone(),
                })
                .unwrap();
        }

        fn unknown_reserved_profiles(
            &mut self,
            config_file: &Utf8Path,
            _workspace_root: &Utf8Path,
            tool: Option<&str>,
            profiles: &[&str],
        ) {
            self.reserved_profiles
                .insert_unique(ReservedProfiles {
                    tool: tool.map(|s| s.to_owned()),
                    config_file: config_file.to_owned(),
                    profiles: profiles.iter().map(|&s| s.to_owned()).collect(),
                })
                .unwrap();
        }

        fn empty_script_sections(
            &mut self,
            config_file: &Utf8Path,
            _workspace_root: &Utf8Path,
            tool: Option<&str>,
            profile_name: &str,
            empty_count: usize,
        ) {
            self.empty_script_warnings
                .insert_unique(EmptyScriptSections {
                    tool: tool.map(|s| s.to_owned()),
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
            tool: Option<&str>,
        ) {
            self.deprecated_scripts
                .insert_unique(DeprecatedScripts {
                    tool: tool.map(|s| s.to_owned()),
                    config_file: config_file.to_owned(),
                })
                .unwrap();
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct UnknownKeys {
        tool: Option<String>,
        config_file: Utf8PathBuf,
        keys: BTreeSet<String>,
    }

    impl IdHashItem for UnknownKeys {
        type Key<'a> = Option<&'a str>;
        fn key(&self) -> Self::Key<'_> {
            self.tool.as_deref()
        }
        id_upcast!();
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct ReservedProfiles {
        tool: Option<String>,
        config_file: Utf8PathBuf,
        profiles: Vec<String>,
    }

    impl IdHashItem for ReservedProfiles {
        type Key<'a> = Option<&'a str>;
        fn key(&self) -> Self::Key<'_> {
            self.tool.as_deref()
        }
        id_upcast!();
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct DeprecatedScripts {
        tool: Option<String>,
        config_file: Utf8PathBuf,
    }

    impl IdHashItem for DeprecatedScripts {
        type Key<'a> = Option<&'a str>;
        fn key(&self) -> Self::Key<'_> {
            self.tool.as_deref()
        }
        id_upcast!();
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct EmptyScriptSections {
        tool: Option<String>,
        config_file: Utf8PathBuf,
        profile_name: String,
        empty_count: usize,
    }

    impl IdHashItem for EmptyScriptSections {
        type Key<'a> = (&'a Option<String>, &'a str);
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
                tool: "my-tool".to_owned(),
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
                    tool: Some("my-tool".to_owned()),
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
                    tool: Some("my-tool".to_owned()),
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
                tool: "tool".to_owned(),
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
                    tool: Some("tool".to_owned()),
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
                tool: "my-tool".to_owned(),
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
                    tool: Some("my-tool".to_owned()),
                    config_file: tool_path,
                }
            }
        );
    }
}
