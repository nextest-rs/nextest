// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::{fmt::Write, sync::LazyLock};

pub(crate) fn short() -> &'static str {
    &VERSION_INFO.short
}

pub(crate) fn long() -> &'static str {
    &VERSION_INFO.long
}

// All the data here is static so we can use a singleton.
static VERSION_INFO: LazyLock<VersionInfo> = LazyLock::new(|| {
    let inner = VersionInfoInner::new();
    let short = inner.to_short();
    let long = inner.to_long();
    VersionInfo { short, long }
});

struct VersionInfo {
    short: String,
    long: String,
}

struct VersionInfoInner {
    /// Nextest's version.
    version: &'static str,

    /// Information about the repository that nextest was built from.
    ///
    /// `None` if the repository information is not available.
    commit_info: Option<CommitInfo>,
}

impl VersionInfoInner {
    const fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            commit_info: CommitInfo::from_env(),
        }
    }

    pub(crate) fn to_short(&self) -> String {
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

    pub(crate) fn to_long(&self) -> String {
        let mut s = self.to_short();
        write!(s, "\nrelease: {}", self.version).unwrap();

        if let Some(commit_info) = &self.commit_info {
            write!(s, "\ncommit-hash: {}", commit_info.commit_hash).unwrap();
            write!(s, "\ncommit-date: {}", commit_info.commit_date).unwrap();
        }
        write!(s, "\nhost: {}", env!("NEXTEST_BUILD_HOST_TARGET")).unwrap();

        // rustc and cargo's version also prints host info here. Unfortunately, clap 4.0's version
        // support only supports a static string rather than a callback, so version info computation
        // can't be deferred. OS info is also quite expensive to compute.
        //
        // For now, just don't do this -- everything else can be statically computed. We'll probably
        // add a dedicated command to collect support data if it's necessary.

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
