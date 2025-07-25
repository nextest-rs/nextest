// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Storage and retrieval of failed test information from previous runs.

use crate::list::TestInstanceId;
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use nextest_metadata::RustBinaryId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;

/// Data about failed tests from the last run, serialized to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedTestsSnapshot {
    /// Version of the snapshot format.
    pub version: u32,

    /// When this snapshot was created.
    pub created_at: DateTime<Utc>,

    /// The profile that was used for this test run.
    pub profile_name: String,

    /// Set of failed tests.
    pub failed_tests: BTreeSet<FailedTest>,
}

/// A single failed test entry.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct FailedTest {
    /// The binary ID.
    pub binary_id: RustBinaryId,

    /// The test name.
    pub test_name: String,
}

impl FailedTest {
    /// Creates a new failed test entry from a test instance ID.
    pub fn from_test_instance_id(id: TestInstanceId<'_>) -> Self {
        Self {
            binary_id: id.binary_id.clone(),
            test_name: id.test_name.to_owned(),
        }
    }
}

/// Manages persistence of failed test information.
pub struct FailedTestStore {
    /// Path to the snapshot file.
    path: Utf8PathBuf,
}

impl FailedTestStore {
    /// Current version of the snapshot format.
    const CURRENT_VERSION: u32 = 1;

    /// Creates a new failed test store with the given path.
    pub fn new(store_dir: &Utf8Path, profile_name: &str) -> Self {
        let path = store_dir.join(format!("{profile_name}-last-failed.json"));
        Self { path }
    }

    /// Loads the failed test snapshot from disk.
    pub fn load(&self) -> Result<Option<FailedTestsSnapshot>, LoadError> {
        match fs::read_to_string(&self.path) {
            Ok(contents) => {
                let snapshot: FailedTestsSnapshot =
                    serde_json::from_str(&contents).map_err(|err| LoadError::DeserializeError {
                        path: self.path.clone(),
                        error: err,
                    })?;

                if snapshot.version != Self::CURRENT_VERSION {
                    return Err(LoadError::VersionMismatch {
                        path: self.path.clone(),
                        expected: Self::CURRENT_VERSION,
                        actual: snapshot.version,
                    });
                }

                Ok(Some(snapshot))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(LoadError::ReadError {
                path: self.path.clone(),
                error: err,
            }),
        }
    }

    /// Saves the failed test snapshot to disk.
    pub fn save(&self, snapshot: &FailedTestsSnapshot) -> Result<(), SaveError> {
        // Ensure the parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| SaveError::CreateDirError {
                path: parent.to_owned(),
                error: err,
            })?;
        }

        let contents = serde_json::to_string_pretty(snapshot)
            .map_err(|err| SaveError::SerializeError { error: err })?;

        fs::write(&self.path, contents).map_err(|err| SaveError::WriteError {
            path: self.path.clone(),
            error: err,
        })?;

        Ok(())
    }

    /// Clears the failed test snapshot by removing the file.
    pub fn clear(&self) -> Result<(), ClearError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(ClearError::RemoveError {
                path: self.path.clone(),
                error: err,
            }),
        }
    }
}

/// Errors that can occur when loading a failed test snapshot.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// Error reading the snapshot file.
    #[error("failed to read snapshot file at {path}")]
    ReadError {
        /// The path that failed to be read.
        path: Utf8PathBuf,
        /// The underlying IO error.
        #[source]
        error: std::io::Error,
    },

    /// Error deserializing the snapshot.
    #[error("failed to deserialize snapshot at {path}")]
    DeserializeError {
        /// The path that failed to be deserialized.
        path: Utf8PathBuf,
        /// The underlying deserialization error.
        #[source]
        error: serde_json::Error,
    },

    /// Version mismatch in the snapshot file.
    #[error("snapshot version mismatch at {path}: expected {expected}, got {actual}")]
    VersionMismatch {
        /// The path with the version mismatch.
        path: Utf8PathBuf,
        /// The expected version.
        expected: u32,
        /// The actual version found.
        actual: u32,
    },
}

/// Errors that can occur when saving a failed test snapshot.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// Error creating the directory.
    #[error("failed to create directory {path}")]
    CreateDirError {
        /// The directory path that failed to be created.
        path: Utf8PathBuf,
        /// The underlying IO error.
        #[source]
        error: std::io::Error,
    },

    /// Error serializing the snapshot.
    #[error("failed to serialize snapshot")]
    SerializeError {
        /// The underlying serialization error.
        #[source]
        error: serde_json::Error,
    },

    /// Error writing the snapshot to disk.
    #[error("failed to write snapshot to {path}")]
    WriteError {
        /// The path that failed to be written.
        path: Utf8PathBuf,
        /// The underlying IO error.
        #[source]
        error: std::io::Error,
    },
}

/// Errors that can occur when clearing a failed test snapshot.
#[derive(Debug, thiserror::Error)]
pub enum ClearError {
    /// Error removing the snapshot file.
    #[error("failed to remove snapshot file at {path}")]
    RemoveError {
        /// The path that failed to be removed.
        path: Utf8PathBuf,
        /// The underlying IO error.
        #[source]
        error: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino_tempfile::Utf8TempDir;

    #[test]
    fn test_store_lifecycle() {
        let temp_dir = Utf8TempDir::new().unwrap();
        let store = FailedTestStore::new(temp_dir.path(), "default");

        // Initially, there should be no snapshot
        assert!(store.load().unwrap().is_none());

        // Create and save a snapshot
        let snapshot = FailedTestsSnapshot {
            version: FailedTestStore::CURRENT_VERSION,
            created_at: Utc::now(),
            profile_name: "default".to_owned(),
            failed_tests: BTreeSet::from([
                FailedTest {
                    binary_id: RustBinaryId::new("test-package::test-binary"),
                    test_name: "test_foo".to_owned(),
                },
                FailedTest {
                    binary_id: RustBinaryId::new("test-package::test-binary"),
                    test_name: "test_bar".to_owned(),
                },
            ]),
        };

        store.save(&snapshot).unwrap();

        // Load and verify
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.version, snapshot.version);
        assert_eq!(loaded.profile_name, snapshot.profile_name);
        assert_eq!(loaded.failed_tests, snapshot.failed_tests);

        // Clear and verify
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }
}
