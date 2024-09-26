// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fmt::Write;

pub(crate) struct VersionInfo {
    /// Nextest's version.
    version: &'static str,

    /// Information about the Git repository that nextest was built from.
    ///
    /// `None` if the Git repository information is not available.
    commit_info: Option<CommitInfo>,
}

impl VersionInfo {
    pub(crate) const fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            commit_info: CommitInfo::from_env(),
        }
    }

    pub(crate) fn to_short_string(&self) -> String {
        let mut s = self.version.to_string();

        if let Some(commit_info) = &self.commit_info {
            write!(
                s,
                " ({} {})",
                commit_info.short_commit_hash, commit_info.commit_date
            )
            .unwrap();
        }

        s
    }

    pub(crate) fn to_long_string(&self) -> String {
        let mut s = self.to_short_string();
        write!(s, "\nrelease: {}", self.version).unwrap();

        if let Some(commit_info) = &self.commit_info {
            write!(s, "\ncommit-hash: {}", commit_info.commit_hash).unwrap();
            write!(s, "\ncommit-date: {}", commit_info.commit_date).unwrap();
        }
        write!(s, "\nhost: {}", env!("NEXTEST_BUILD_HOST_TARGET")).unwrap();
        write!(s, "\nos: {}", os_info::get()).unwrap();

        s
    }
}

struct CommitInfo {
    short_commit_hash: &'static str,
    commit_hash: &'static str,
    commit_date: &'static str,
}

impl CommitInfo {
    const fn from_env() -> Option<Self> {
        let Some(short_commit_hash) = option_env!("NEXTEST_BUILD_COMMIT_SHORT_HASH") else {
            return None;
        };
        let Some(commit_hash) = option_env!("NEXTEST_BUILD_COMMIT_HASH") else {
            return None;
        };
        let Some(commit_date) = option_env!("NEXTEST_BUILD_COMMIT_DATE") else {
            return None;
        };

        Some(Self {
            short_commit_hash,
            commit_hash,
            commit_date,
        })
    }
}
