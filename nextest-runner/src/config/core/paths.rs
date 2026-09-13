// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! File locations and invocation-relative diagnostics for repository configuration.

use camino::{Utf8Path, Utf8PathBuf};
use camino_anchored::{
    AbsUtf8PathBuf, AnchoredPath, CurrentDirError, DisplayPath, PathAnchor, RelUtf8PathBuf,
    ResolvePathError,
};
use std::sync::Arc;
use thiserror::Error;

/// Tracks the directory from which nextest was invoked.
#[derive(Clone, Debug)]
pub struct InvocationDir(PathAnchor);

impl InvocationDir {
    /// Creates a new `InvocationDir`, capturing the process's current directory.
    pub fn capture() -> Result<Self, ConfigPathResolveError> {
        let anchor = PathAnchor::current_dir().map_err(|error| {
            ConfigPathResolveError::new(".", ConfigPathResolveErrorKind::CurrentDir(error))
        })?;
        Ok(Self(anchor))
    }

    /// Creates a new `InvocationDir` from a provided directory.
    pub fn new(directory: AbsUtf8PathBuf) -> Self {
        Self(PathAnchor::new(directory))
    }

    /// Resolves a user-provided configuration path.
    pub fn resolve_input(&self, path: &Utf8Path) -> Result<ConfigPath, ConfigPathResolveError> {
        let resolved = self.0.resolve_input(path).map_err(|error| {
            ConfigPathResolveError::new(path, ConfigPathResolveErrorKind::Input(error))
        })?;
        Ok(ConfigPath(Arc::new(resolved)))
    }

    fn resolve_absolute(&self, path: AbsUtf8PathBuf) -> ConfigPath {
        ConfigPath(Arc::new(self.0.resolve_absolute(path)))
    }
}

/// Tracks the workspace directory used for config discovery, including
/// workspace remapping.
#[derive(Clone, Debug)]
pub struct WorkspaceRoot(AbsUtf8PathBuf);

impl WorkspaceRoot {
    /// Uses an absolute workspace directory.
    pub fn new(path: AbsUtf8PathBuf) -> Self {
        Self(path)
    }

    /// Returns the absolute workspace directory.
    pub fn as_path(&self) -> &Utf8Path {
        self.0.as_path()
    }
}

/// Separate invocation and workspace directories shared by both config loaders.
#[derive(Clone, Debug)]
pub struct ConfigPaths {
    invocation: InvocationDir,
    workspace_root: WorkspaceRoot,
}

impl ConfigPaths {
    /// Captures the invocation directory and resolves the workspace directory.
    pub fn capture(workspace_root: impl Into<Utf8PathBuf>) -> Result<Self, ConfigPathResolveError> {
        let invocation = InvocationDir::capture()?;
        let workspace_root = invocation.resolve_input(&workspace_root.into())?;
        Ok(Self::new(
            invocation,
            WorkspaceRoot::new(workspace_root.0.absolute().clone()),
        ))
    }

    /// Uses explicit invocation and workspace directories.
    pub fn new(invocation: InvocationDir, workspace_root: WorkspaceRoot) -> Self {
        Self {
            invocation,
            workspace_root,
        }
    }

    /// Returns the workspace directory for discovery.
    pub fn workspace_root(&self) -> &WorkspaceRoot {
        &self.workspace_root
    }

    /// Resolves an explicit config input against the invocation directory.
    pub fn resolve_input(&self, path: &Utf8Path) -> Result<ConfigPath, ConfigPathResolveError> {
        self.invocation.resolve_input(path)
    }

    /// Locates a repository config file relative to the workspace directory.
    pub fn repository_config(&self, relative: &RelUtf8PathBuf) -> ConfigPath {
        self.invocation
            .resolve_absolute(self.workspace_root.0.join(relative))
    }
}

/// A config file's absolute location, as well as invocation-specific diagnostic
/// spelling.
#[derive(Clone, Debug)]
pub struct ConfigPath(Arc<AnchoredPath>);

impl ConfigPath {
    /// Returns the absolute file location for I/O.
    pub fn absolute_path(&self) -> &Utf8Path {
        self.0.absolute().as_path()
    }

    /// Displays the path relative to the invocation directory when possible.
    pub fn display(&self) -> DisplayPath<'_> {
        self.0.display()
    }
}

impl PartialEq for ConfigPath {
    fn eq(&self, other: &Self) -> bool {
        self.0.absolute() == other.0.absolute()
    }
}

impl Eq for ConfigPath {}

/// An error establishing a configuration file's absolute location.
#[derive(Debug, Error)]
#[error("could not resolve configuration path `{path}`")]
pub struct ConfigPathResolveError {
    /// The input that could not be resolved.
    pub path: Utf8PathBuf,
    /// The reason resolution failed.
    #[source]
    pub error: ConfigPathResolveErrorKind,
}

impl ConfigPathResolveError {
    fn new(path: impl Into<Utf8PathBuf>, error: ConfigPathResolveErrorKind) -> Self {
        Self {
            path: path.into(),
            error,
        }
    }
}

/// The reason a configuration path could not be resolved.
#[derive(Debug, Error)]
pub enum ConfigPathResolveErrorKind {
    /// The current (invocation) directory could not be determined.
    #[error(transparent)]
    CurrentDir(CurrentDirError),

    /// An input path could not be resolved against the invocation directory.
    #[error(transparent)]
    Input(ResolvePathError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino_tempfile::tempdir;

    #[test]
    fn invocation_and_workspace_are_independent() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        for cwd in [
            workspace.clone(),
            workspace.join("member"),
            temp.path().to_owned(),
        ] {
            let paths = ConfigPaths::new(
                InvocationDir::new(AbsUtf8PathBuf::new(cwd.clone()).unwrap()),
                WorkspaceRoot::new(AbsUtf8PathBuf::new(workspace.clone()).unwrap()),
            );
            let repository =
                paths.repository_config(&RelUtf8PathBuf::new(".config/nextest.toml").unwrap());
            assert_eq!(
                repository.absolute_path(),
                workspace.join(".config/nextest.toml")
            );
            let expected = repository
                .absolute_path()
                .strip_prefix(&cwd)
                .unwrap_or(repository.absolute_path());
            assert_eq!(repository.display().to_string(), expected.as_str());
            let explicit = paths.resolve_input(Utf8Path::new("./custom.toml")).unwrap();
            assert_eq!(explicit.absolute_path(), cwd.join("./custom.toml"));
            assert_eq!(explicit.display().to_string(), "./custom.toml");
        }
    }
}
