// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration file discovery shared by the early and full loaders.

use super::{ConfigPath, ConfigPaths, ToolConfigFile, ToolName};
use crate::errors::{ConfigParseError, ConfigParseErrorKind};
use camino::Utf8Path;
use std::{fs, io};
use tracing::debug;

/// Selects repository configuration files independently of tool configuration.
#[derive(Clone, Copy, Debug)]
pub struct ConfigFileSelection<'a> {
    /// An explicit, required configuration file, suppressing automatic discovery.
    pub config_file: Option<&'a Utf8Path>,
}

impl<'a> ConfigFileSelection<'a> {
    /// Creates a new `ConfigFileSelection` that uses an explicit file if given,
    /// otherwise the repository file at the workspace root.
    pub fn new(config_file: Option<&'a Utf8Path>) -> Self {
        Self { config_file }
    }

    /// Returns the explicit repository config file, if one was selected.
    pub fn explicit_config_file(self) -> Option<&'a Utf8Path> {
        self.config_file
    }

    /// Resolves the explicit repository input or discovers the shared file.
    pub(super) fn repo_config_path(
        self,
        paths: &ConfigPaths,
    ) -> Result<ConfigPath, ConfigParseError> {
        match self.config_file {
            Some(path) => Ok(paths.resolve_input(path)?),
            None => Ok(paths.shared_config()),
        }
    }

    /// Returns every config file to load, lowest priority first.
    ///
    /// Returns tool files in the given (already assumed to be reversed) order,
    /// then the repository file.
    pub(super) fn sources<'t>(
        self,
        paths: &ConfigPaths,
        tool_config_files_rev: impl Iterator<Item = &'t ToolConfigFile>,
    ) -> Result<Vec<ConfigSource<'t>>, ConfigParseError> {
        let mut sources: Vec<_> = tool_config_files_rev
            .map(|ToolConfigFile { config_file, tool }| {
                Ok(ConfigSource {
                    path: paths.resolve_input(config_file)?,
                    kind: ConfigSourceKind::Tool(tool),
                })
            })
            .collect::<Result<_, ConfigParseError>>()?;
        let kind = match self.config_file {
            Some(_) => ConfigSourceKind::ExplicitRepository,
            None => ConfigSourceKind::DiscoveredRepository,
        };
        sources.push(ConfigSource {
            path: self.repo_config_path(paths)?,
            kind,
        });
        Ok(sources)
    }
}

pub(super) struct ConfigSource<'t> {
    pub(super) path: ConfigPath,
    pub(super) kind: ConfigSourceKind<'t>,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ConfigSourceKind<'t> {
    Tool(&'t ToolName),
    ExplicitRepository,
    DiscoveredRepository,
}

impl<'t> ConfigSource<'t> {
    pub(super) fn tool(&self) -> Option<&'t ToolName> {
        match self.kind {
            ConfigSourceKind::Tool(tool) => Some(tool),
            ConfigSourceKind::ExplicitRepository | ConfigSourceKind::DiscoveredRepository => None,
        }
    }

    pub(super) fn required(&self) -> bool {
        match self.kind {
            ConfigSourceKind::Tool(_) | ConfigSourceKind::ExplicitRepository => true,
            ConfigSourceKind::DiscoveredRepository => false,
        }
    }

    /// Reads this configuration file.
    ///
    /// Returns `None` only for an absent optional file.
    pub(super) fn read(&self) -> Result<Option<String>, ConfigParseError> {
        match fs::read_to_string(self.path.absolute_path()) {
            Ok(contents) => {
                debug!(
                    config_file = %self.path.display(),
                    tool = self.tool().map(ToolName::as_str),
                    "read config file",
                );
                Ok(Some(contents))
            }
            // A regular file named `.config` yields NotADirectory on Unix and
            // NotFound on Windows -- both mean that the optional file is
            // absent.
            Err(error) if !self.required() => match error.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                    debug!(config_file = %self.path.display(), "config file not found, skipping");
                    Ok(None)
                }
                _ => Err(self.read_error(error)),
            },
            Err(error) => Err(self.read_error(error)),
        }
    }

    fn read_error(&self, error: io::Error) -> ConfigParseError {
        ConfigParseError::new(
            &self.path,
            self.tool(),
            ConfigParseErrorKind::ReadError(error),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        core::{NextestConfig, VersionOnlyConfig},
        utils::test_helpers::*,
    };
    use camino_tempfile::{Utf8TempDir, tempdir};
    use camino_tempfile_ext::prelude::*;
    use nextest_filtering::ParseContext;
    use std::collections::BTreeSet;
    use test_case::test_case;

    #[derive(Clone, Copy, Debug)]
    enum Loader {
        VersionOnly,
        Full,
    }

    #[derive(Clone, Copy, Debug)]
    enum Scenario {
        AbsentRepoConfig,
        DotConfigIsFile,
        RepoConfigIsDirectory,
        MissingToolConfig,
        MissingExplicitConfig,
    }

    fn tool_name(s: &str) -> ToolName {
        ToolName::new(s.into()).unwrap()
    }

    // temp_workspace always writes .config/nextest.toml, so remove it
    // afterwards to get a workspace without a repo config.
    fn workspace_without_repo_config(dir: &Utf8TempDir) -> guppy::graph::PackageGraph {
        let graph = temp_workspace(dir, "");
        fs::remove_dir_all(dir.child(".config")).unwrap();
        graph
    }

    fn load(
        loader: Loader,
        dir: &Utf8TempDir,
        graph: &guppy::graph::PackageGraph,
        config_file: Option<&Utf8Path>,
        tool_config_files: &[ToolConfigFile],
    ) -> Result<(), ConfigParseError> {
        match loader {
            Loader::VersionOnly => {
                VersionOnlyConfig::from_sources(dir.path(), config_file, tool_config_files)
                    .map(|_| ())
            }
            Loader::Full => NextestConfig::from_sources(
                dir.path(),
                &ParseContext::new(graph),
                config_file,
                tool_config_files,
                &BTreeSet::new(),
            )
            .map(|_| ()),
        }
    }

    #[test_case(Loader::VersionOnly, Scenario::AbsentRepoConfig; "version only, absent repo config")]
    #[test_case(Loader::Full, Scenario::AbsentRepoConfig; "full, absent repo config")]
    #[test_case(Loader::VersionOnly, Scenario::DotConfigIsFile; "version only, .config is a file")]
    #[test_case(Loader::Full, Scenario::DotConfigIsFile; "full, .config is a file")]
    #[test_case(Loader::VersionOnly, Scenario::RepoConfigIsDirectory; "version only, repo config is a directory")]
    #[test_case(Loader::Full, Scenario::RepoConfigIsDirectory; "full, repo config is a directory")]
    #[test_case(Loader::VersionOnly, Scenario::MissingToolConfig; "version only, missing tool config")]
    #[test_case(Loader::Full, Scenario::MissingToolConfig; "full, missing tool config")]
    #[test_case(Loader::VersionOnly, Scenario::MissingExplicitConfig; "version only, missing explicit config")]
    #[test_case(Loader::Full, Scenario::MissingExplicitConfig; "full, missing explicit config")]
    fn read_errors_and_absent_files(loader: Loader, scenario: Scenario) {
        let dir = tempdir().unwrap();
        let graph = workspace_without_repo_config(&dir);
        let repo_config = dir.child(NextestConfig::CONFIG_PATH);

        match scenario {
            Scenario::AbsentRepoConfig => {
                load(loader, &dir, &graph, None, &[]).expect("absent repo config is optional");
            }
            Scenario::DotConfigIsFile => {
                dir.child(".config").write_str("not a directory").unwrap();
                load(loader, &dir, &graph, None, &[])
                    .expect("a regular file named .config means the repo config is absent");
            }
            Scenario::RepoConfigIsDirectory => {
                repo_config.create_dir_all().unwrap();
                let error = load(loader, &dir, &graph, None, &[]).unwrap_err();
                assert_eq!(error.config_file(), repo_config.as_path());
                assert_eq!(error.tool(), None);
                let ConfigParseErrorKind::ReadError(_) = error.kind() else {
                    panic!("a directory at the repo config path is a read error, got {error:?}");
                };
            }
            Scenario::MissingToolConfig => {
                let tool = ToolConfigFile {
                    tool: tool_name("missing-tool"),
                    config_file: dir.child("missing-tool.toml").to_path_buf(),
                };
                let error =
                    load(loader, &dir, &graph, None, std::slice::from_ref(&tool)).unwrap_err();
                assert_eq!(error.config_file(), tool.config_file);
                assert_eq!(error.tool(), Some(&tool.tool));
                let ConfigParseErrorKind::ReadError(_) = error.kind() else {
                    panic!("a missing tool config file is a read error, got {error:?}");
                };
            }
            Scenario::MissingExplicitConfig => {
                let explicit = dir.child("missing.toml");
                let error = load(loader, &dir, &graph, Some(explicit.as_path()), &[]).unwrap_err();
                assert_eq!(error.config_file(), explicit.as_path());
                assert_eq!(error.tool(), None);
                let ConfigParseErrorKind::ReadError(_) = error.kind() else {
                    panic!("a missing explicit config file is a read error, got {error:?}");
                };
            }
        }
    }
}
