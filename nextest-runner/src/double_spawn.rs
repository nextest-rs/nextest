// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for double-spawning test processes.
//!
//! Nextest has experimental support on Unix for spawning test processes twice, to enable better
//! isolation and solve some thorny issues.
//!
//! ## Issues this currently solves
//!
//! ### `posix_spawn` SIGTSTP race
//!
//! It's been empirically observed that if nextest receives a `SIGTSTP` (Ctrl-Z) while it's running,
//! it can get completely stuck sometimes. This is due to a race between the child being spawned and it
//! receiving a `SIGTSTP` signal.
//!
//! For more details, see [this
//! message](https://sourceware.org/pipermail/libc-help/2022-August/006263.html) on the glibc-help
//! mailing list.
//!
//! To solve this issue, we do the following:
//!
//! 1. In the main nextest runner process, using `DoubleSpawnContext`, block `SIGTSTP` in the
//!    current thread (using `pthread_sigmask`) before spawning the stub child cargo-nextest
//!    process.
//! 2. In the stub child process, unblock `SIGTSTP`.
//!
//! With this approach, the race condition between posix_spawn and `SIGTSTP` no longer exists.

use std::path::Path;

/// Information about double-spawning processes. This determines whether a process will be
/// double-spawned.
///
/// This is used by the main nextest process.
#[derive(Clone, Debug)]
pub struct DoubleSpawnInfo {
    inner: imp::DoubleSpawnInfo,
}

impl DoubleSpawnInfo {
    /// The name of the double-spawn subcommand, used throughout nextest.
    pub const SUBCOMMAND_NAME: &'static str = "__double-spawn";

    /// Attempts to enable double-spawning and returns a new `DoubleSpawnInfo`.
    ///
    /// If double-spawning is not available, [`current_exe`](Self::current_exe) returns `None`.
    pub fn try_enable() -> Self {
        Self {
            inner: imp::DoubleSpawnInfo::try_enable(),
        }
    }

    /// Returns a `DoubleSpawnInfo` which doesn't perform any double-spawning.
    pub fn disabled() -> Self {
        Self {
            inner: imp::DoubleSpawnInfo::disabled(),
        }
    }

    /// Returns the current executable, if one is available.
    ///
    /// If `None`, double-spawning is not used.
    pub fn current_exe(&self) -> Option<&Path> {
        self.inner.current_exe()
    }

    /// Returns a context that is meant to be obtained before spawning processes and dropped afterwards.
    pub fn spawn_context(&self) -> Option<DoubleSpawnContext> {
        self.current_exe().map(|_| DoubleSpawnContext::new())
    }
}

/// Context to be used before spawning processes and dropped afterwards.
///
/// Returned by [`DoubleSpawnInfo::spawn_context`].
#[derive(Debug)]
pub struct DoubleSpawnContext {
    // Only used for the Drop impl.
    #[expect(dead_code)]
    inner: imp::DoubleSpawnContext,
}

impl DoubleSpawnContext {
    #[inline]
    fn new() -> Self {
        Self {
            inner: imp::DoubleSpawnContext::new(),
        }
    }

    /// Close the double-spawn context, dropping any changes that needed to be done to it.
    pub fn finish(self) {}
}

/// Initialization for the double-spawn child.
pub fn double_spawn_child_init() {
    imp::double_spawn_child_init()
}

#[cfg(unix)]
mod imp {
    use super::*;
    use nix::sys::signal::{SigSet, Signal};
    use std::path::PathBuf;
    use tracing::warn;

    #[derive(Clone, Debug)]
    pub(super) struct DoubleSpawnInfo {
        current_exe: Option<PathBuf>,
    }

    impl DoubleSpawnInfo {
        #[inline]
        pub(super) fn try_enable() -> Self {
            // Attempt to obtain the current exe, and warn if it couldn't be found.
            // TODO: maybe add an option to fail?
            // TODO: Always use /proc/self/exe directly on Linux, just make sure it's always accessible
            let current_exe = get_current_exe().map_or_else(
                |error| {
                    warn!(
                        "unable to determine current exe, will not use double-spawning \
                        for better isolation: {error}"
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

    #[cfg(target_os = "linux")]
    fn get_current_exe() -> std::io::Result<PathBuf> {
        static PROC_SELF_EXE: &str = "/proc/self/exe";

        // Always use /proc/self/exe directly rather than trying to readlink it. Just make sure it's
        // accessible.
        let path = Path::new(PROC_SELF_EXE);
        match path.symlink_metadata() {
            Ok(_) => Ok(path.to_owned()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(std::io::Error::other(
                "no /proc/self/exe available. Is /proc mounted?",
            )),
            Err(e) => Err(e),
        }
    }

    // TODO: Add other symlinks as well, e.g. /proc/self/path/a.out on solaris/illumos.

    #[cfg(not(target_os = "linux"))]
    #[inline]
    fn get_current_exe() -> std::io::Result<PathBuf> {
        std::env::current_exe()
    }

    #[derive(Debug)]
    pub(super) struct DoubleSpawnContext {
        to_unblock: Option<SigSet>,
    }

    impl DoubleSpawnContext {
        #[inline]
        pub(super) fn new() -> Self {
            // Block SIGTSTP, unblocking it in the child process. This avoids a complex race
            // condition.
            let mut sigset = SigSet::empty();
            sigset.add(Signal::SIGTSTP);
            let to_unblock = sigset.thread_block().ok().map(|()| sigset);
            Self { to_unblock }
        }
    }

    impl Drop for DoubleSpawnContext {
        fn drop(&mut self) {
            if let Some(sigset) = &self.to_unblock {
                _ = sigset.thread_unblock();
            }
        }
    }

    #[inline]
    pub(super) fn double_spawn_child_init() {
        let mut sigset = SigSet::empty();
        sigset.add(Signal::SIGTSTP);
        if sigset.thread_unblock().is_err() {
            warn!("[double-spawn] unable to unblock SIGTSTP in child");
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    #[derive(Clone, Debug)]
    pub(super) struct DoubleSpawnInfo {}

    impl DoubleSpawnInfo {
        #[inline]
        pub(super) fn try_enable() -> Self {
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

    #[derive(Debug)]
    pub(super) struct DoubleSpawnContext {}

    impl DoubleSpawnContext {
        #[inline]
        pub(super) fn new() -> Self {
            Self {}
        }
    }

    #[inline]
    pub(super) fn double_spawn_child_init() {}
}
