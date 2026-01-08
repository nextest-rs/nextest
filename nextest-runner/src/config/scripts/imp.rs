// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Setup scripts.

use crate::{
    config::{
        core::{ConfigIdentifier, EvaluatableProfile, FinalConfig, PreBuildPlatform},
        elements::{LeakTimeout, SlowTimeout},
        overrides::{MaybeTargetSpec, PlatformStrings},
    },
    double_spawn::{DoubleSpawnContext, DoubleSpawnInfo},
    errors::{
        ChildStartError, ConfigCompileError, ConfigCompileErrorKind, ConfigCompileSection,
        InvalidConfigScriptName,
    },
    helpers::convert_rel_path_to_main_sep,
    list::TestList,
    platform::BuildPlatforms,
    reporter::events::SetupScriptEnvMap,
    test_command::{apply_ld_dyld_env, create_command},
};
use camino::Utf8Path;
use camino_tempfile::Utf8TempPath;
use guppy::graph::cargo::BuildPlatform;
use iddqd::{IdOrdItem, id_upcast};
use indexmap::IndexMap;
use nextest_filtering::{
    BinaryQuery, EvalContext, Filterset, FiltersetKind, ParseContext, TestQuery,
};
use quick_junit::ReportUuid;
use serde::{Deserialize, de::Error};
use smol_str::SmolStr;
use std::{
    collections::{HashMap, HashSet},
    fmt,
    process::Command,
    sync::Arc,
};
use swrite::{SWrite, swrite};

/// The scripts defined in nextest configuration.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ScriptConfig {
    // These maps are ordered because scripts are used in the order they're defined.
    /// The setup scripts defined in nextest's configuration.
    #[serde(default)]
    pub setup: IndexMap<ScriptId, SetupScriptConfig>,
    /// The wrapper scripts defined in nextest's configuration.
    #[serde(default)]
    pub wrapper: IndexMap<ScriptId, WrapperScriptConfig>,
}

impl ScriptConfig {
    pub(in crate::config) fn is_empty(&self) -> bool {
        self.setup.is_empty() && self.wrapper.is_empty()
    }

    /// Returns information about the script with the given ID.
    ///
    /// Panics if the ID is invalid.
    pub(in crate::config) fn script_info(&self, id: ScriptId) -> ScriptInfo {
        let script_type = if self.setup.contains_key(&id) {
            ScriptType::Setup
        } else if self.wrapper.contains_key(&id) {
            ScriptType::Wrapper
        } else {
            panic!("ScriptConfig::script_info called with invalid script ID: {id}")
        };

        ScriptInfo {
            id: id.clone(),
            script_type,
        }
    }

    /// Returns an iterator over the names of all scripts of all types.
    pub(in crate::config) fn all_script_ids(&self) -> impl Iterator<Item = &ScriptId> {
        self.setup.keys().chain(self.wrapper.keys())
    }

    /// Returns an iterator over names that are used by more than one type of
    /// script.
    pub(in crate::config) fn duplicate_ids(&self) -> impl Iterator<Item = &ScriptId> {
        self.wrapper.keys().filter(|k| self.setup.contains_key(*k))
    }
}

/// Basic information about a script, used during error checking.
#[derive(Clone, Debug)]
pub struct ScriptInfo {
    /// The script ID.
    pub id: ScriptId,

    /// The type of the script.
    pub script_type: ScriptType,
}

impl IdOrdItem for ScriptInfo {
    type Key<'a> = &'a ScriptId;
    fn key(&self) -> Self::Key<'_> {
        &self.id
    }
    id_upcast!();
}

/// The script type as configured in the `[scripts]` table.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum ScriptType {
    /// A setup script.
    Setup,

    /// A wrapper script.
    Wrapper,
}

impl ScriptType {
    pub(in crate::config) fn matches(self, profile_script_type: ProfileScriptType) -> bool {
        match self {
            ScriptType::Setup => profile_script_type == ProfileScriptType::Setup,
            ScriptType::Wrapper => {
                profile_script_type == ProfileScriptType::ListWrapper
                    || profile_script_type == ProfileScriptType::RunWrapper
            }
        }
    }
}

impl fmt::Display for ScriptType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ScriptType::Setup => f.write_str("setup"),
            ScriptType::Wrapper => f.write_str("wrapper"),
        }
    }
}

/// A script type as configured in `[[profile.*.scripts]]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileScriptType {
    /// A setup script.
    Setup,

    /// A list-time wrapper script.
    ListWrapper,

    /// A run-time wrapper script.
    RunWrapper,
}

impl fmt::Display for ProfileScriptType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ProfileScriptType::Setup => f.write_str("setup"),
            ProfileScriptType::ListWrapper => f.write_str("list-wrapper"),
            ProfileScriptType::RunWrapper => f.write_str("run-wrapper"),
        }
    }
}

/// Data about setup scripts, returned by an [`EvaluatableProfile`].
pub struct SetupScripts<'profile> {
    enabled_scripts: IndexMap<&'profile ScriptId, SetupScript<'profile>>,
}

impl<'profile> SetupScripts<'profile> {
    pub(in crate::config) fn new(
        profile: &'profile EvaluatableProfile<'_>,
        test_list: &TestList<'_>,
    ) -> Self {
        Self::new_with_queries(
            profile,
            test_list
                .iter_tests()
                .filter(|test| test.test_info.filter_match.is_match())
                .map(|test| test.to_test_query()),
        )
    }

    // Creates a new `SetupScripts` instance for the given profile and matching tests.
    fn new_with_queries<'a>(
        profile: &'profile EvaluatableProfile<'_>,
        matching_tests: impl IntoIterator<Item = TestQuery<'a>>,
    ) -> Self {
        let script_config = profile.script_config();
        let profile_scripts = &profile.compiled_data.scripts;
        if profile_scripts.is_empty() {
            return Self {
                enabled_scripts: IndexMap::new(),
            };
        }

        // Build a map of setup scripts to the test configurations that enable them.
        let mut by_script_id = HashMap::new();
        for profile_script in profile_scripts {
            for script_id in &profile_script.setup {
                by_script_id
                    .entry(script_id)
                    .or_insert_with(Vec::new)
                    .push(profile_script);
            }
        }

        let env = profile.filterset_ecx();

        // This is a map from enabled setup scripts to a list of configurations that enabled them.
        let mut enabled_ids = HashSet::new();
        for test in matching_tests {
            // Look at all the setup scripts activated by this test.
            for (&script_id, compiled) in &by_script_id {
                if enabled_ids.contains(script_id) {
                    // This script is already enabled.
                    continue;
                }
                if compiled.iter().any(|data| data.is_enabled(&test, &env)) {
                    enabled_ids.insert(script_id);
                }
            }
        }

        // Build up a map of enabled scripts along with their data, by script ID.
        let mut enabled_scripts = IndexMap::new();
        for (script_id, config) in &script_config.setup {
            if enabled_ids.contains(script_id) {
                let compiled = by_script_id
                    .remove(script_id)
                    .expect("script id must be present");
                enabled_scripts.insert(
                    script_id,
                    SetupScript {
                        id: script_id.clone(),
                        config,
                        compiled,
                    },
                );
            }
        }

        Self { enabled_scripts }
    }

    /// Returns the number of enabled setup scripts.
    #[inline]
    pub fn len(&self) -> usize {
        self.enabled_scripts.len()
    }

    /// Returns true if there are no enabled setup scripts.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.enabled_scripts.is_empty()
    }

    /// Returns enabled setup scripts in the order they should be run in.
    #[inline]
    pub(crate) fn into_iter(self) -> impl Iterator<Item = SetupScript<'profile>> {
        self.enabled_scripts.into_values()
    }
}

/// Data about an individual setup script.
///
/// Returned by [`SetupScripts::iter`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub(crate) struct SetupScript<'profile> {
    /// The script ID.
    pub(crate) id: ScriptId,

    /// The configuration for the script.
    pub(crate) config: &'profile SetupScriptConfig,

    /// The compiled filters to use to check which tests this script is enabled for.
    pub(crate) compiled: Vec<&'profile CompiledProfileScripts<FinalConfig>>,
}

impl SetupScript<'_> {
    pub(crate) fn is_enabled(&self, test: &TestQuery<'_>, cx: &EvalContext<'_>) -> bool {
        self.compiled
            .iter()
            .any(|compiled| compiled.is_enabled(test, cx))
    }
}

/// Represents a to-be-run setup script command with a certain set of arguments.
pub(crate) struct SetupScriptCommand {
    /// The command to be run.
    command: std::process::Command,
    /// The environment file.
    env_path: Utf8TempPath,
    /// Double-spawn context.
    double_spawn: Option<DoubleSpawnContext>,
}

impl SetupScriptCommand {
    /// Creates a new `SetupScriptCommand` for a setup script.
    pub(crate) fn new(
        config: &SetupScriptConfig,
        profile_name: &str,
        double_spawn: &DoubleSpawnInfo,
        test_list: &TestList<'_>,
    ) -> Result<Self, ChildStartError> {
        let mut cmd = create_command(
            config.command.program(
                test_list.workspace_root(),
                &test_list.rust_build_meta().target_directory,
            ),
            &config.command.args,
            double_spawn,
        );

        // NB: we will always override user-provided environment variables with the
        // `CARGO_*` and `NEXTEST_*` variables set directly on `cmd` below.
        test_list.cargo_env().apply_env(&mut cmd);

        let env_path = camino_tempfile::Builder::new()
            .prefix("nextest-env")
            .tempfile()
            .map_err(|error| ChildStartError::TempPath(Arc::new(error)))?
            .into_temp_path();

        cmd.current_dir(test_list.workspace_root())
            // This environment variable is set to indicate that tests are being run under nextest.
            .env("NEXTEST", "1")
            // Set the nextest profile.
            .env("NEXTEST_PROFILE", profile_name)
            // Setup scripts can define environment variables which are written out here.
            .env("NEXTEST_ENV", &env_path);

        apply_ld_dyld_env(&mut cmd, test_list.updated_dylib_path());

        let double_spawn = double_spawn.spawn_context();

        Ok(Self {
            command: cmd,
            env_path,
            double_spawn,
        })
    }

    /// Returns the command to be run.
    #[inline]
    pub(crate) fn command_mut(&mut self) -> &mut std::process::Command {
        &mut self.command
    }

    pub(crate) fn spawn(self) -> std::io::Result<(tokio::process::Child, Utf8TempPath)> {
        let mut command = tokio::process::Command::from(self.command);
        let res = command.spawn();
        if let Some(ctx) = self.double_spawn {
            ctx.finish();
        }
        let child = res?;
        Ok((child, self.env_path))
    }
}

/// Data obtained by executing setup scripts. This is used to set up the environment for tests.
#[derive(Clone, Debug, Default)]
pub(crate) struct SetupScriptExecuteData<'profile> {
    env_maps: Vec<(SetupScript<'profile>, SetupScriptEnvMap)>,
}

impl<'profile> SetupScriptExecuteData<'profile> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn add_script(&mut self, script: SetupScript<'profile>, env_map: SetupScriptEnvMap) {
        self.env_maps.push((script, env_map));
    }

    /// Applies the data from setup scripts to the given test instance.
    pub(crate) fn apply(&self, test: &TestQuery<'_>, cx: &EvalContext<'_>, command: &mut Command) {
        for (script, env_map) in &self.env_maps {
            if script.is_enabled(test, cx) {
                for (key, value) in env_map.env_map.iter() {
                    command.env(key, value);
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledProfileScripts<State> {
    pub(in crate::config) setup: Vec<ScriptId>,
    pub(in crate::config) list_wrapper: Option<ScriptId>,
    pub(in crate::config) run_wrapper: Option<ScriptId>,
    pub(in crate::config) data: ProfileScriptData,
    pub(in crate::config) state: State,
}

impl CompiledProfileScripts<PreBuildPlatform> {
    pub(in crate::config) fn new(
        pcx: &ParseContext<'_>,
        profile_name: &str,
        index: usize,
        source: &DeserializedProfileScriptConfig,
        errors: &mut Vec<ConfigCompileError>,
    ) -> Option<Self> {
        if source.platform.host.is_none()
            && source.platform.target.is_none()
            && source.filter.is_none()
        {
            errors.push(ConfigCompileError {
                profile_name: profile_name.to_owned(),
                section: ConfigCompileSection::Script(index),
                kind: ConfigCompileErrorKind::ConstraintsNotSpecified {
                    // The default filter is not relevant for scripts -- it is a
                    // configuration value, not a constraint.
                    default_filter_specified: false,
                },
            });
            return None;
        }

        let host_spec = MaybeTargetSpec::new(source.platform.host.as_deref());
        let target_spec = MaybeTargetSpec::new(source.platform.target.as_deref());

        let filter_expr = source.filter.as_ref().map_or(Ok(None), |filter| {
            // TODO: probably want to restrict the set of expressions here via
            // the `kind` parameter.
            Some(Filterset::parse(
                filter.clone(),
                pcx,
                FiltersetKind::DefaultFilter,
            ))
            .transpose()
        });

        match (host_spec, target_spec, filter_expr) {
            (Ok(host_spec), Ok(target_spec), Ok(expr)) => Some(Self {
                setup: source.setup.clone(),
                list_wrapper: source.list_wrapper.clone(),
                run_wrapper: source.run_wrapper.clone(),
                data: ProfileScriptData {
                    host_spec,
                    target_spec,
                    expr,
                },
                state: PreBuildPlatform {},
            }),
            (maybe_host_err, maybe_platform_err, maybe_parse_err) => {
                let host_platform_parse_error = maybe_host_err.err();
                let platform_parse_error = maybe_platform_err.err();
                let parse_errors = maybe_parse_err.err();

                errors.push(ConfigCompileError {
                    profile_name: profile_name.to_owned(),
                    section: ConfigCompileSection::Script(index),
                    kind: ConfigCompileErrorKind::Parse {
                        host_parse_error: host_platform_parse_error,
                        target_parse_error: platform_parse_error,
                        filter_parse_errors: parse_errors.into_iter().collect(),
                    },
                });
                None
            }
        }
    }

    pub(in crate::config) fn apply_build_platforms(
        self,
        build_platforms: &BuildPlatforms,
    ) -> CompiledProfileScripts<FinalConfig> {
        let host_eval = self.data.host_spec.eval(&build_platforms.host.platform);
        let host_test_eval = self.data.target_spec.eval(&build_platforms.host.platform);
        let target_eval = build_platforms
            .target
            .as_ref()
            .map_or(host_test_eval, |target| {
                self.data.target_spec.eval(&target.triple.platform)
            });

        CompiledProfileScripts {
            setup: self.setup,
            list_wrapper: self.list_wrapper,
            run_wrapper: self.run_wrapper,
            data: self.data,
            state: FinalConfig {
                host_eval,
                host_test_eval,
                target_eval,
            },
        }
    }
}

impl CompiledProfileScripts<FinalConfig> {
    pub(in crate::config) fn is_enabled_binary(
        &self,
        query: &BinaryQuery<'_>,
        cx: &EvalContext<'_>,
    ) -> Option<bool> {
        if !self.state.host_eval {
            return Some(false);
        }
        if query.platform == BuildPlatform::Host && !self.state.host_test_eval {
            return Some(false);
        }
        if query.platform == BuildPlatform::Target && !self.state.target_eval {
            return Some(false);
        }

        if let Some(expr) = &self.data.expr {
            expr.matches_binary(query, cx)
        } else {
            Some(true)
        }
    }

    pub(in crate::config) fn is_enabled(
        &self,
        query: &TestQuery<'_>,
        cx: &EvalContext<'_>,
    ) -> bool {
        if !self.state.host_eval {
            return false;
        }
        if query.binary_query.platform == BuildPlatform::Host && !self.state.host_test_eval {
            return false;
        }
        if query.binary_query.platform == BuildPlatform::Target && !self.state.target_eval {
            return false;
        }

        if let Some(expr) = &self.data.expr {
            expr.matches_test(query, cx)
        } else {
            true
        }
    }
}

/// The name of a configuration script.
#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct ScriptId(pub ConfigIdentifier);

impl ScriptId {
    /// Creates a new script identifier.
    pub fn new(identifier: SmolStr) -> Result<Self, InvalidConfigScriptName> {
        let identifier = ConfigIdentifier::new(identifier).map_err(InvalidConfigScriptName)?;
        Ok(Self(identifier))
    }

    /// Returns the name of the script as a [`ConfigIdentifier`].
    pub fn as_identifier(&self) -> &ConfigIdentifier {
        &self.0
    }

    /// Returns a unique ID for this script, consisting of the run ID, the script ID, and the stress index.
    pub fn unique_id(&self, run_id: ReportUuid, stress_index: Option<u32>) -> String {
        let mut out = String::new();
        swrite!(out, "{run_id}:{self}");
        if let Some(stress_index) = stress_index {
            swrite!(out, "@stress-{}", stress_index);
        }
        out
    }

    #[cfg(test)]
    pub(super) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for ScriptId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Try and deserialize as a string.
        let identifier = SmolStr::deserialize(deserializer)?;
        Self::new(identifier).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for ScriptId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug)]
pub(in crate::config) struct ProfileScriptData {
    host_spec: MaybeTargetSpec,
    target_spec: MaybeTargetSpec,
    expr: Option<Filterset>,
}

impl ProfileScriptData {
    pub(in crate::config) fn expr(&self) -> Option<&Filterset> {
        self.expr.as_ref()
    }
}

/// Deserialized form of profile-specific script configuration before compilation.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::config) struct DeserializedProfileScriptConfig {
    /// The host and/or target platforms to match against.
    #[serde(default)]
    pub(in crate::config) platform: PlatformStrings,

    /// The filterset to match against.
    #[serde(default)]
    filter: Option<String>,

    /// The setup script or scripts to run.
    #[serde(default, deserialize_with = "deserialize_script_ids")]
    setup: Vec<ScriptId>,

    /// The wrapper script to run at list time.
    #[serde(default)]
    list_wrapper: Option<ScriptId>,

    /// The wrapper script to run at run time.
    #[serde(default)]
    run_wrapper: Option<ScriptId>,
}

/// Deserialized form of setup script configuration before compilation.
///
/// This is defined as a top-level element.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SetupScriptConfig {
    /// The command to run. The first element is the program and the second element is a list
    /// of arguments.
    pub command: ScriptCommand,

    /// An optional slow timeout for this command.
    #[serde(
        default,
        deserialize_with = "crate::config::elements::deserialize_slow_timeout"
    )]
    pub slow_timeout: Option<SlowTimeout>,

    /// An optional leak timeout for this command.
    #[serde(
        default,
        deserialize_with = "crate::config::elements::deserialize_leak_timeout"
    )]
    pub leak_timeout: Option<LeakTimeout>,

    /// Whether to capture standard output for this command.
    #[serde(default)]
    pub capture_stdout: bool,

    /// Whether to capture standard error for this command.
    #[serde(default)]
    pub capture_stderr: bool,

    /// JUnit configuration for this script.
    #[serde(default)]
    pub junit: SetupScriptJunitConfig,
}

impl SetupScriptConfig {
    /// Returns true if at least some output isn't being captured.
    #[inline]
    pub fn no_capture(&self) -> bool {
        !(self.capture_stdout && self.capture_stderr)
    }
}

/// A JUnit override configuration.
#[derive(Copy, Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SetupScriptJunitConfig {
    /// Whether to store successful output.
    ///
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub store_success_output: bool,

    /// Whether to store failing output.
    ///
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub store_failure_output: bool,
}

impl Default for SetupScriptJunitConfig {
    fn default() -> Self {
        Self {
            store_success_output: true,
            store_failure_output: true,
        }
    }
}

/// Deserialized form of wrapper script configuration before compilation.
///
/// This is defined as a top-level element.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct WrapperScriptConfig {
    /// The command to run.
    pub command: ScriptCommand,

    /// How this script interacts with a configured target runner, if any.
    /// Defaults to ignoring the target runner.
    #[serde(default)]
    pub target_runner: WrapperScriptTargetRunner,
}

/// Interaction of wrapper script with a configured target runner.
#[derive(Clone, Debug, Default)]
pub enum WrapperScriptTargetRunner {
    /// The target runner is ignored. This is the default.
    #[default]
    Ignore,

    /// The target runner overrides the wrapper.
    OverridesWrapper,

    /// The target runner runs within the wrapper script. The command line used
    /// is `<wrapper> <target-runner> <test-binary> <args>`.
    WithinWrapper,

    /// The target runner runs around the wrapper script. The command line used
    /// is `<target-runner> <wrapper> <test-binary> <args>`.
    AroundWrapper,
}

impl<'de> Deserialize<'de> for WrapperScriptTargetRunner {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "ignore" => Ok(WrapperScriptTargetRunner::Ignore),
            "overrides-wrapper" => Ok(WrapperScriptTargetRunner::OverridesWrapper),
            "within-wrapper" => Ok(WrapperScriptTargetRunner::WithinWrapper),
            "around-wrapper" => Ok(WrapperScriptTargetRunner::AroundWrapper),
            _ => Err(serde::de::Error::unknown_variant(
                &s,
                &[
                    "ignore",
                    "overrides-wrapper",
                    "within-wrapper",
                    "around-wrapper",
                ],
            )),
        }
    }
}

fn default_true() -> bool {
    true
}

fn deserialize_script_ids<'de, D>(deserializer: D) -> Result<Vec<ScriptId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct ScriptIdVisitor;

    impl<'de> serde::de::Visitor<'de> for ScriptIdVisitor {
        type Value = Vec<ScriptId>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a script ID (string) or a list of script IDs")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(vec![ScriptId::new(value.into()).map_err(E::custom)?])
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut ids = Vec::new();
            while let Some(value) = seq.next_element::<String>()? {
                ids.push(ScriptId::new(value.into()).map_err(A::Error::custom)?);
            }
            Ok(ids)
        }
    }

    deserializer.deserialize_any(ScriptIdVisitor)
}

/// The script command to run.
#[derive(Clone, Debug)]
pub struct ScriptCommand {
    /// The program to run.
    pub program: String,

    /// The arguments to pass to the program.
    pub args: Vec<String>,

    /// Which directory to interpret the program as relative to.
    ///
    /// This controls just how `program` is interpreted, in case it is a
    /// relative path.
    pub relative_to: ScriptCommandRelativeTo,
}

impl ScriptCommand {
    /// Returns the program to run, resolved with respect to the target directory.
    pub fn program(&self, workspace_root: &Utf8Path, target_dir: &Utf8Path) -> String {
        match self.relative_to {
            ScriptCommandRelativeTo::None => self.program.clone(),
            ScriptCommandRelativeTo::WorkspaceRoot => {
                // If the path is relative, convert it to the main separator.
                let path = Utf8Path::new(&self.program);
                if path.is_relative() {
                    workspace_root
                        .join(convert_rel_path_to_main_sep(path))
                        .to_string()
                } else {
                    path.to_string()
                }
            }
            ScriptCommandRelativeTo::Target => {
                // If the path is relative, convert it to the main separator.
                let path = Utf8Path::new(&self.program);
                if path.is_relative() {
                    target_dir
                        .join(convert_rel_path_to_main_sep(path))
                        .to_string()
                } else {
                    path.to_string()
                }
            }
        }
    }
}

impl<'de> Deserialize<'de> for ScriptCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CommandVisitor;

        impl<'de> serde::de::Visitor<'de> for CommandVisitor {
            type Value = ScriptCommand;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a Unix shell command, a list of arguments, or a table with command-line and relative-to")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let mut args = shell_words::split(value).map_err(E::custom)?;
                if args.is_empty() {
                    return Err(E::invalid_value(serde::de::Unexpected::Str(value), &self));
                }
                let program = args.remove(0);
                Ok(ScriptCommand {
                    program,
                    args,
                    relative_to: ScriptCommandRelativeTo::None,
                })
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let Some(program) = seq.next_element::<String>()? else {
                    return Err(A::Error::invalid_length(0, &self));
                };
                let mut args = Vec::new();
                while let Some(value) = seq.next_element::<String>()? {
                    args.push(value);
                }
                Ok(ScriptCommand {
                    program,
                    args,
                    relative_to: ScriptCommandRelativeTo::None,
                })
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut command_line = None;
                let mut relative_to = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "command-line" => {
                            if command_line.is_some() {
                                return Err(A::Error::duplicate_field("command-line"));
                            }
                            command_line = Some(map.next_value_seed(CommandInnerSeed)?);
                        }
                        "relative-to" => {
                            if relative_to.is_some() {
                                return Err(A::Error::duplicate_field("relative-to"));
                            }
                            relative_to = Some(map.next_value::<ScriptCommandRelativeTo>()?);
                        }
                        _ => {
                            return Err(A::Error::unknown_field(
                                &key,
                                &["command-line", "relative-to"],
                            ));
                        }
                    }
                }

                let (program, arguments) =
                    command_line.ok_or_else(|| A::Error::missing_field("command-line"))?;
                let relative_to = relative_to.unwrap_or(ScriptCommandRelativeTo::None);

                Ok(ScriptCommand {
                    program,
                    args: arguments,
                    relative_to,
                })
            }
        }

        deserializer.deserialize_any(CommandVisitor)
    }
}

struct CommandInnerSeed;

impl<'de> serde::de::DeserializeSeed<'de> for CommandInnerSeed {
    type Value = (String, Vec<String>);

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CommandInnerVisitor;

        impl<'de> serde::de::Visitor<'de> for CommandInnerVisitor {
            type Value = (String, Vec<String>);

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string or array of strings")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let mut args = shell_words::split(value).map_err(E::custom)?;
                if args.is_empty() {
                    return Err(E::invalid_value(
                        serde::de::Unexpected::Str(value),
                        &"a non-empty command string",
                    ));
                }
                let program = args.remove(0);
                Ok((program, args))
            }

            fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
            where
                S: serde::de::SeqAccess<'de>,
            {
                let mut args = Vec::new();
                while let Some(value) = seq.next_element::<String>()? {
                    args.push(value);
                }
                if args.is_empty() {
                    return Err(S::Error::invalid_length(0, &self));
                }
                let program = args.remove(0);
                Ok((program, args))
            }
        }

        deserializer.deserialize_any(CommandInnerVisitor)
    }
}

/// The directory to interpret a [`ScriptCommand`] as relative to, in case it is
/// a relative path.
///
/// If specified, the program will be joined with the provided path.
#[derive(Clone, Copy, Debug)]
pub enum ScriptCommandRelativeTo {
    /// Do not join the program with any path.
    None,

    /// Join the program with the workspace root.
    WorkspaceRoot,

    /// Join the program with the target directory.
    Target,
    // TODO: TargetProfile, similar to ArchiveRelativeTo
}

impl<'de> Deserialize<'de> for ScriptCommandRelativeTo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "none" => Ok(ScriptCommandRelativeTo::None),
            "workspace-root" => Ok(ScriptCommandRelativeTo::WorkspaceRoot),
            "target" => Ok(ScriptCommandRelativeTo::Target),
            _ => Err(serde::de::Error::unknown_variant(&s, &["none", "target"])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{
            core::{ConfigExperimental, NextestConfig, ToolConfigFile, ToolName},
            utils::test_helpers::*,
        },
        errors::{
            ConfigParseErrorKind, DisplayErrorChain, ProfileListScriptUsesRunFiltersError,
            ProfileScriptErrors, ProfileUnknownScriptError, ProfileWrongConfigScriptTypeError,
        },
    };
    use camino_tempfile::tempdir;
    use camino_tempfile_ext::prelude::*;
    use indoc::indoc;
    use maplit::btreeset;
    use nextest_metadata::TestCaseName;
    use test_case::test_case;

    fn tool_name(s: &str) -> ToolName {
        ToolName::new(s.into()).unwrap()
    }

    #[test]
    fn test_scripts_basic() {
        let config_contents = indoc! {r#"
            [[profile.default.scripts]]
            platform = { host = "x86_64-unknown-linux-gnu" }
            filter = "test(script1)"
            setup = ["foo", "bar"]

            [[profile.default.scripts]]
            platform = { target = "aarch64-apple-darwin" }
            filter = "test(script2)"
            setup = "baz"

            [[profile.default.scripts]]
            filter = "test(script3)"
            # No matter which order scripts are specified here, they must always be run in the
            # order defined below.
            setup = ["baz", "foo", "@tool:my-tool:toolscript"]

            [scripts.setup.foo]
            command = "command foo"

            [scripts.setup.bar]
            command = ["cargo", "run", "-p", "bar"]
            slow-timeout = { period = "60s", terminate-after = 2 }

            [scripts.setup.baz]
            command = "baz"
            slow-timeout = "1s"
            leak-timeout = "1s"
            capture-stdout = true
            capture-stderr = true
        "#
        };

        let tool_config_contents = indoc! {r#"
            [scripts.setup.'@tool:my-tool:toolscript']
            command = "tool-command"
            "#
        };

        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);
        let tool_path = workspace_dir.child(".config/my-tool.toml");
        tool_path.write_str(tool_config_contents).unwrap();

        let package_id = graph.workspace().iter().next().unwrap().id();

        let pcx = ParseContext::new(&graph);

        let tool_config_files = [ToolConfigFile {
            tool: tool_name("my-tool"),
            config_file: tool_path.to_path_buf(),
        }];

        // First, check that if the experimental feature isn't enabled, we get an error.
        let nextest_config_error = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &tool_config_files,
            &Default::default(),
        )
        .unwrap_err();
        match nextest_config_error.kind() {
            ConfigParseErrorKind::ExperimentalFeaturesNotEnabled { missing_features } => {
                assert_eq!(
                    *missing_features,
                    btreeset! { ConfigExperimental::SetupScripts }
                );
            }
            other => panic!("unexpected error kind: {other:?}"),
        }

        // Now, check with the experimental feature enabled.
        let nextest_config_result = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &tool_config_files,
            &btreeset! { ConfigExperimental::SetupScripts },
        )
        .expect("config is valid");
        let profile = nextest_config_result
            .profile("default")
            .expect("valid profile name")
            .apply_build_platforms(&build_platforms());

        // This query matches the foo and bar scripts.
        let host_binary_query =
            binary_query(&graph, package_id, "lib", "my-binary", BuildPlatform::Host);
        let test_name = TestCaseName::new("script1");
        let query = TestQuery {
            binary_query: host_binary_query.to_query(),
            test_name: &test_name,
        };
        let scripts = SetupScripts::new_with_queries(&profile, std::iter::once(query));
        assert_eq!(scripts.len(), 2, "two scripts should be enabled");
        assert_eq!(
            scripts.enabled_scripts.get_index(0).unwrap().0.as_str(),
            "foo",
            "first script should be foo"
        );
        assert_eq!(
            scripts.enabled_scripts.get_index(1).unwrap().0.as_str(),
            "bar",
            "second script should be bar"
        );

        let target_binary_query = binary_query(
            &graph,
            package_id,
            "lib",
            "my-binary",
            BuildPlatform::Target,
        );

        // This query matches the baz script.
        let test_name = TestCaseName::new("script2");
        let query = TestQuery {
            binary_query: target_binary_query.to_query(),
            test_name: &test_name,
        };
        let scripts = SetupScripts::new_with_queries(&profile, std::iter::once(query));
        assert_eq!(scripts.len(), 1, "one script should be enabled");
        assert_eq!(
            scripts.enabled_scripts.get_index(0).unwrap().0.as_str(),
            "baz",
            "first script should be baz"
        );

        // This query matches the baz, foo and tool scripts (but note the order).
        let test_name = TestCaseName::new("script3");
        let query = TestQuery {
            binary_query: target_binary_query.to_query(),
            test_name: &test_name,
        };
        let scripts = SetupScripts::new_with_queries(&profile, std::iter::once(query));
        assert_eq!(scripts.len(), 3, "three scripts should be enabled");
        assert_eq!(
            scripts.enabled_scripts.get_index(0).unwrap().0.as_str(),
            "@tool:my-tool:toolscript",
            "first script should be toolscript"
        );
        assert_eq!(
            scripts.enabled_scripts.get_index(1).unwrap().0.as_str(),
            "foo",
            "second script should be foo"
        );
        assert_eq!(
            scripts.enabled_scripts.get_index(2).unwrap().0.as_str(),
            "baz",
            "third script should be baz"
        );
    }

    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = ""
        "#},
        "invalid value: string \"\", expected a Unix shell command, a list of arguments, \
         or a table with command-line and relative-to"

        ; "empty command"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = []
        "#},
        "invalid length 0, expected a Unix shell command, a list of arguments, \
         or a table with command-line and relative-to"

        ; "empty command list"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
        "#},
        r#"scripts.setup.foo: missing configuration field "scripts.setup.foo.command""#

        ; "missing command"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = { command-line = "" }
        "#},
        "invalid value: string \"\", expected a non-empty command string"

        ; "empty command-line in table"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = { command-line = [] }
        "#},
        "invalid length 0, expected a string or array of strings"

        ; "empty command-line array in table"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = { relative-to = "target" }
        "#},
        r#"missing configuration field "scripts.setup.foo.command.command-line""#

        ; "missing command-line in table"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = { command-line = "my-command", relative-to = "invalid" }
        "#},
        r#"unknown variant `invalid`, expected `none` or `target`"#

        ; "invalid relative-to value"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = { command-line = "my-command", unknown-field = "value" }
        "#},
        r#"unknown field `unknown-field`, expected `command-line` or `relative-to`"#

        ; "unknown field in command table"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = "my-command"
            slow-timeout = 34
        "#},
        r#"invalid type: integer `34`, expected a table ({ period = "60s", terminate-after = 2 }) or a string ("60s")"#

        ; "slow timeout is not a duration"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.'@tool:foo']
            command = "my-command"
        "#},
        r#"invalid configuration script name: tool identifier not of the form "@tool:tool-name:identifier": `@tool:foo`"#

        ; "invalid tool script name"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.'#foo']
            command = "my-command"
        "#},
        r"invalid configuration script name: invalid identifier `#foo`"

        ; "invalid script name"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.wrapper.foo]
            command = "my-command"
            target-runner = "not-a-valid-value"
        "#},
        r#"unknown variant `not-a-valid-value`, expected one of `ignore`, `overrides-wrapper`, `within-wrapper`, `around-wrapper`"#

        ; "invalid target-runner value"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.wrapper.foo]
            command = "my-command"
            target-runner = ["foo"]
        "#},
        r#"invalid type: sequence, expected a string"#

        ; "target-runner is not a string"
    )]
    fn parse_scripts_invalid_deserialize(config_contents: &str, message: &str) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);
        let pcx = ParseContext::new(&graph);

        let nextest_config_error = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &btreeset! { ConfigExperimental::SetupScripts, ConfigExperimental::WrapperScripts },
        )
        .expect_err("config is invalid");
        let actual_message = DisplayErrorChain::new(nextest_config_error).to_string();

        assert!(
            actual_message.contains(message),
            "nextest config error `{actual_message}` contains message `{message}`"
        );
    }

    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = "my-command"

            [[profile.default.scripts]]
            setup = ["foo"]
        "#},
        "default",
        &[MietteJsonReport {
            message: "at least one of `platform` and `filter` must be specified".to_owned(),
            labels: vec![],
        }]

        ; "neither platform nor filter specified"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = "my-command"

            [[profile.default.scripts]]
            platform = {}
            setup = ["foo"]
        "#},
        "default",
        &[MietteJsonReport {
            message: "at least one of `platform` and `filter` must be specified".to_owned(),
            labels: vec![],
        }]

        ; "empty platform map"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = "my-command"

            [[profile.default.scripts]]
            platform = { host = 'cfg(target_os = "linux' }
            setup = ["foo"]
        "#},
        "default",
        &[MietteJsonReport {
            message: "error parsing cfg() expression".to_owned(),
            labels: vec![
                MietteJsonLabel { label: "expected one of `=`, `,`, `)` here".to_owned(), span: MietteJsonSpan { offset: 3, length: 1 } }
            ]
        }]

        ; "invalid platform expression"
    )]
    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = "my-command"

            [[profile.ci.overrides]]
            filter = 'test(/foo)'
            setup = ["foo"]
        "#},
        "ci",
        &[MietteJsonReport {
            message: "expected close regex".to_owned(),
            labels: vec![
                MietteJsonLabel { label: "missing `/`".to_owned(), span: MietteJsonSpan { offset: 9, length: 0 } }
            ]
        }]

        ; "invalid filterset"
    )]
    fn parse_scripts_invalid_compile(
        config_contents: &str,
        faulty_profile: &str,
        expected_reports: &[MietteJsonReport],
    ) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);

        let pcx = ParseContext::new(&graph);

        let error = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &btreeset! { ConfigExperimental::SetupScripts, ConfigExperimental::WrapperScripts },
        )
        .expect_err("config is invalid");
        match error.kind() {
            ConfigParseErrorKind::CompileErrors(compile_errors) => {
                assert_eq!(
                    compile_errors.len(),
                    1,
                    "exactly one override error must be produced"
                );
                let error = compile_errors.first().unwrap();
                assert_eq!(
                    error.profile_name, faulty_profile,
                    "compile error profile matches"
                );
                let handler = miette::JSONReportHandler::new();
                let reports = error
                    .kind
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
                panic!(
                    "for config error {other:?}, expected ConfigParseErrorKind::CompiledDataParseError"
                );
            }
        }
    }

    #[test_case(
        indoc! {r#"
            [scripts.setup.'@tool:foo:bar']
            command = "my-command"

            [[profile.ci.overrides]]
            setup = ["@tool:foo:bar"]
        "#},
        &["@tool:foo:bar"]

        ; "tool config in main program")]
    fn parse_scripts_invalid_defined(config_contents: &str, expected_invalid_scripts: &[&str]) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);

        let pcx = ParseContext::new(&graph);

        let error = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &btreeset! { ConfigExperimental::SetupScripts, ConfigExperimental::WrapperScripts },
        )
        .expect_err("config is invalid");
        match error.kind() {
            ConfigParseErrorKind::InvalidConfigScriptsDefined(scripts) => {
                assert_eq!(
                    scripts.len(),
                    expected_invalid_scripts.len(),
                    "correct number of scripts defined"
                );
                for (script, expected_script) in scripts.iter().zip(expected_invalid_scripts) {
                    assert_eq!(script.as_str(), *expected_script, "script name matches");
                }
            }
            other => {
                panic!(
                    "for config error {other:?}, expected ConfigParseErrorKind::InvalidConfigScriptsDefined"
                );
            }
        }
    }

    #[test_case(
        indoc! {r#"
            [scripts.setup.'blarg']
            command = "my-command"

            [[profile.ci.overrides]]
            setup = ["blarg"]
        "#},
        &["blarg"]

        ; "non-tool config in tool")]
    fn parse_scripts_invalid_defined_by_tool(
        tool_config_contents: &str,
        expected_invalid_scripts: &[&str],
    ) {
        let workspace_dir = tempdir().unwrap();
        let graph = temp_workspace(&workspace_dir, "");

        let tool_path = workspace_dir.child(".config/my-tool.toml");
        tool_path.write_str(tool_config_contents).unwrap();
        let tool_config_files = [ToolConfigFile {
            tool: tool_name("my-tool"),
            config_file: tool_path.to_path_buf(),
        }];

        let pcx = ParseContext::new(&graph);

        let error = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &tool_config_files,
            &btreeset! { ConfigExperimental::SetupScripts },
        )
        .expect_err("config is invalid");
        match error.kind() {
            ConfigParseErrorKind::InvalidConfigScriptsDefinedByTool(scripts) => {
                assert_eq!(
                    scripts.len(),
                    expected_invalid_scripts.len(),
                    "exactly one script must be defined"
                );
                for (script, expected_script) in scripts.iter().zip(expected_invalid_scripts) {
                    assert_eq!(script.as_str(), *expected_script, "script name matches");
                }
            }
            other => {
                panic!(
                    "for config error {other:?}, expected ConfigParseErrorKind::InvalidConfigScriptsDefinedByTool"
                );
            }
        }
    }

    #[test_case(
        indoc! {r#"
            [scripts.setup.foo]
            command = 'echo foo'

            [[profile.default.scripts]]
            platform = 'cfg(unix)'
            setup = ['bar']

            [[profile.ci.scripts]]
            platform = 'cfg(unix)'
            setup = ['baz']
        "#},
        vec![
            ProfileUnknownScriptError {
                profile_name: "default".to_owned(),
                name: ScriptId::new("bar".into()).unwrap(),
            },
            ProfileUnknownScriptError {
                profile_name: "ci".to_owned(),
                name: ScriptId::new("baz".into()).unwrap(),
            },
        ],
        &["foo"]

        ; "unknown scripts"
    )]
    fn parse_scripts_invalid_unknown(
        config_contents: &str,
        expected_errors: Vec<ProfileUnknownScriptError>,
        expected_known_scripts: &[&str],
    ) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);

        let pcx = ParseContext::new(&graph);

        let error = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &btreeset! { ConfigExperimental::SetupScripts, ConfigExperimental::WrapperScripts },
        )
        .expect_err("config is invalid");
        match error.kind() {
            ConfigParseErrorKind::ProfileScriptErrors {
                errors,
                known_scripts,
            } => {
                let ProfileScriptErrors {
                    unknown_scripts,
                    wrong_script_types,
                    list_scripts_using_run_filters,
                } = &**errors;
                assert_eq!(wrong_script_types.len(), 0, "no wrong script types");
                assert_eq!(
                    list_scripts_using_run_filters.len(),
                    0,
                    "no scripts using run filters in list phase"
                );
                assert_eq!(
                    unknown_scripts.len(),
                    expected_errors.len(),
                    "correct number of errors"
                );
                for (error, expected_error) in unknown_scripts.iter().zip(expected_errors) {
                    assert_eq!(error, &expected_error, "error matches");
                }
                assert_eq!(
                    known_scripts.len(),
                    expected_known_scripts.len(),
                    "correct number of known scripts"
                );
                for (script, expected_script) in known_scripts.iter().zip(expected_known_scripts) {
                    assert_eq!(
                        script.as_str(),
                        *expected_script,
                        "known script name matches"
                    );
                }
            }
            other => {
                panic!(
                    "for config error {other:?}, expected ConfigParseErrorKind::ProfileScriptErrors"
                );
            }
        }
    }

    #[test_case(
        indoc! {r#"
            [scripts.setup.setup-script]
            command = 'echo setup'

            [scripts.wrapper.wrapper-script]
            command = 'echo wrapper'

            [[profile.default.scripts]]
            platform = 'cfg(unix)'
            setup = ['wrapper-script']
            list-wrapper = 'setup-script'

            [[profile.ci.scripts]]
            platform = 'cfg(unix)'
            setup = 'wrapper-script'
            run-wrapper = 'setup-script'
        "#},
        vec![
            ProfileWrongConfigScriptTypeError {
                profile_name: "default".to_owned(),
                name: ScriptId::new("wrapper-script".into()).unwrap(),
                attempted: ProfileScriptType::Setup,
                actual: ScriptType::Wrapper,
            },
            ProfileWrongConfigScriptTypeError {
                profile_name: "default".to_owned(),
                name: ScriptId::new("setup-script".into()).unwrap(),
                attempted: ProfileScriptType::ListWrapper,
                actual: ScriptType::Setup,
            },
            ProfileWrongConfigScriptTypeError {
                profile_name: "ci".to_owned(),
                name: ScriptId::new("wrapper-script".into()).unwrap(),
                attempted: ProfileScriptType::Setup,
                actual: ScriptType::Wrapper,
            },
            ProfileWrongConfigScriptTypeError {
                profile_name: "ci".to_owned(),
                name: ScriptId::new("setup-script".into()).unwrap(),
                attempted: ProfileScriptType::RunWrapper,
                actual: ScriptType::Setup,
            },
        ],
        &["setup-script", "wrapper-script"]

        ; "wrong script types"
    )]
    fn parse_scripts_invalid_wrong_type(
        config_contents: &str,
        expected_errors: Vec<ProfileWrongConfigScriptTypeError>,
        expected_known_scripts: &[&str],
    ) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);

        let pcx = ParseContext::new(&graph);

        let error = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &btreeset! { ConfigExperimental::SetupScripts, ConfigExperimental::WrapperScripts },
        )
        .expect_err("config is invalid");
        match error.kind() {
            ConfigParseErrorKind::ProfileScriptErrors {
                errors,
                known_scripts,
            } => {
                let ProfileScriptErrors {
                    unknown_scripts,
                    wrong_script_types,
                    list_scripts_using_run_filters,
                } = &**errors;
                assert_eq!(unknown_scripts.len(), 0, "no unknown scripts");
                assert_eq!(
                    list_scripts_using_run_filters.len(),
                    0,
                    "no scripts using run filters in list phase"
                );
                assert_eq!(
                    wrong_script_types.len(),
                    expected_errors.len(),
                    "correct number of errors"
                );
                for (error, expected_error) in wrong_script_types.iter().zip(expected_errors) {
                    assert_eq!(error, &expected_error, "error matches");
                }
                assert_eq!(
                    known_scripts.len(),
                    expected_known_scripts.len(),
                    "correct number of known scripts"
                );
                for (script, expected_script) in known_scripts.iter().zip(expected_known_scripts) {
                    assert_eq!(
                        script.as_str(),
                        *expected_script,
                        "known script name matches"
                    );
                }
            }
            other => {
                panic!(
                    "for config error {other:?}, expected ConfigParseErrorKind::ProfileScriptErrors"
                );
            }
        }
    }

    #[test_case(
        indoc! {r#"
            [scripts.wrapper.list-script]
            command = 'echo list'

            [[profile.default.scripts]]
            filter = 'test(hello)'
            list-wrapper = 'list-script'

            [[profile.ci.scripts]]
            filter = 'test(world)'
            list-wrapper = 'list-script'
        "#},
        vec![
            ProfileListScriptUsesRunFiltersError {
                profile_name: "default".to_owned(),
                name: ScriptId::new("list-script".into()).unwrap(),
                script_type: ProfileScriptType::ListWrapper,
                filters: vec!["test(hello)".to_owned()].into_iter().collect(),
            },
            ProfileListScriptUsesRunFiltersError {
                profile_name: "ci".to_owned(),
                name: ScriptId::new("list-script".into()).unwrap(),
                script_type: ProfileScriptType::ListWrapper,
                filters: vec!["test(world)".to_owned()].into_iter().collect(),
            },
        ],
        &["list-script"]

        ; "list scripts using run filters"
    )]
    fn parse_scripts_invalid_list_using_run_filters(
        config_contents: &str,
        expected_errors: Vec<ProfileListScriptUsesRunFiltersError>,
        expected_known_scripts: &[&str],
    ) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);

        let pcx = ParseContext::new(&graph);

        let error = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &btreeset! { ConfigExperimental::SetupScripts, ConfigExperimental::WrapperScripts },
        )
        .expect_err("config is invalid");
        match error.kind() {
            ConfigParseErrorKind::ProfileScriptErrors {
                errors,
                known_scripts,
            } => {
                let ProfileScriptErrors {
                    unknown_scripts,
                    wrong_script_types,
                    list_scripts_using_run_filters,
                } = &**errors;
                assert_eq!(unknown_scripts.len(), 0, "no unknown scripts");
                assert_eq!(wrong_script_types.len(), 0, "no wrong script types");
                assert_eq!(
                    list_scripts_using_run_filters.len(),
                    expected_errors.len(),
                    "correct number of errors"
                );
                for (error, expected_error) in
                    list_scripts_using_run_filters.iter().zip(expected_errors)
                {
                    assert_eq!(error, &expected_error, "error matches");
                }
                assert_eq!(
                    known_scripts.len(),
                    expected_known_scripts.len(),
                    "correct number of known scripts"
                );
                for (script, expected_script) in known_scripts.iter().zip(expected_known_scripts) {
                    assert_eq!(
                        script.as_str(),
                        *expected_script,
                        "known script name matches"
                    );
                }
            }
            other => {
                panic!(
                    "for config error {other:?}, expected ConfigParseErrorKind::ProfileScriptErrors"
                );
            }
        }
    }

    #[test]
    fn test_parse_scripts_empty_sections() {
        let config_contents = indoc! {r#"
            [scripts.setup.foo]
            command = 'echo foo'

            [[profile.default.scripts]]
            platform = 'cfg(unix)'

            [[profile.ci.scripts]]
            platform = 'cfg(unix)'
        "#};

        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);

        let pcx = ParseContext::new(&graph);

        // The config should still be valid, just with warnings
        let result = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &btreeset! { ConfigExperimental::SetupScripts, ConfigExperimental::WrapperScripts },
        );

        match result {
            Ok(_config) => {
                // Config should be valid, warnings are just printed to stderr
                // The warnings we added should have been printed during config parsing
            }
            Err(e) => {
                panic!("Config should be valid but got error: {e:?}");
            }
        }
    }
}
