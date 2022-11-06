// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for double-spawning test processes.
//!
//! Nextest has experimental support on Unix for spawning test processes twice, to enable better
//! isolation and solve some thorny issues.

use self::imp::DoubleSpawnInfoImp;
use std::path::Path;

/// Information about double-spawning processes. This determines whether a process will be
/// double-spawned.
///
/// This is used by the main nextest process.
#[derive(Clone, Debug)]
pub struct DoubleSpawnInfo {
    inner: DoubleSpawnInfoImp,
}

impl DoubleSpawnInfo {
    /// The name of the double-spawn subcommand, used throughout nextest.
    pub const SUBCOMMAND_NAME: &'static str = "__double-spawn";

    /// This returns a `DoubleSpawnInfo` which attempts to perform double-spawning.
    ///
    /// This is super experimental, and should be used with caution.
    pub fn enabled() -> Self {
        Self {
            inner: DoubleSpawnInfoImp::enabled(),
        }
    }

    /// This returns a `DoubleSpawnInfo` which disables double-spawning.
    pub fn disabled() -> Self {
        Self {
            inner: DoubleSpawnInfoImp::disabled(),
        }
    }
    /// Returns the current executable, if one is available.
    ///
    /// If `None`, double-spawning is not used.
    pub fn current_exe(&self) -> Option<&Path> {
        self.inner.current_exe()
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::path::PathBuf;

    #[derive(Clone, Debug)]
    pub(super) struct DoubleSpawnInfoImp {
        current_exe: Option<PathBuf>,
    }

    impl DoubleSpawnInfoImp {
        #[inline]
        pub(super) fn enabled() -> Self {
            // Attempt to obtain the current exe, and warn if it couldn't be found.
            // TODO: maybe add an option to fail?
            let current_exe = std::env::current_exe().map_or_else(
                |error| {
                    log::warn!(
                        "unable to determine current exe, tests will be less isolated: {error}"
                    );
                    None
                },
                Some,
            );
            Self { current_exe }
        }

        #[inline]
        pub(super) fn disabled() -> Self {
            Self { current_exe: None }
        }

        #[inline]
        pub(super) fn current_exe(&self) -> Option<&Path> {
            self.current_exe.as_deref()
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    #[derive(Clone, Debug)]
    pub(super) struct DoubleSpawnInfoImp {}

    impl DoubleSpawnInfoImp {
        #[inline]
        pub(super) fn enabled() -> Self {
            Self {}
        }

        #[inline]
        pub(super) fn disabled() -> Self {
            Self {}
        }

        #[inline]
        pub(super) fn current_exe(&self) -> Option<&Path> {
            None
        }
    }
}
