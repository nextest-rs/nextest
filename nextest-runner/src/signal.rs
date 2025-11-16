// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for handling signals in nextest.

use crate::errors::SignalHandlerSetupError;

/// The kind of signal handling to set up for a test run.
///
/// A `SignalHandlerKind` can be passed into
/// [`TestRunnerBuilder::build`](crate::runner::TestRunnerBuilder::build).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum SignalHandlerKind {
    /// The standard signal handler. Capture interrupt and termination signals depending on the
    /// platform.
    Standard,

    /// Debugger mode signal handler. Only handles termination signals (SIGTERM,
    /// SIGHUP) to allow graceful cleanup. Other signals are ignored by nextest
    /// and are expected to be handled by the debugger.
    DebuggerMode,

    /// A no-op signal handler. Useful for tests.
    Noop,
}

impl SignalHandlerKind {
    pub(crate) fn build(self) -> Result<SignalHandler, SignalHandlerSetupError> {
        match self {
            Self::Standard => SignalHandler::new(),
            Self::DebuggerMode => SignalHandler::debugger_mode(),
            Self::Noop => Ok(SignalHandler::noop()),
        }
    }
}

/// The signal handler implementation.
#[derive(Debug)]
pub(crate) struct SignalHandler {
    signals: Option<imp::Signals>,
}

impl SignalHandler {
    /// Creates a new `SignalHandler` that handles Ctrl-C and other signals.
    #[cfg(any(unix, windows))]
    pub(crate) fn new() -> Result<Self, SignalHandlerSetupError> {
        let signals = imp::Signals::new()?;
        Ok(Self {
            signals: Some(signals),
        })
    }

    /// Creates a new `SignalHandler` for debugger mode that only handles termination signals.
    #[cfg(any(unix, windows))]
    pub(crate) fn debugger_mode() -> Result<Self, SignalHandlerSetupError> {
        let signals = imp::Signals::debugger_mode()?;
        Ok(Self {
            signals: Some(signals),
        })
    }

    /// Creates a new `SignalReceiver` that does nothing.
    pub(crate) fn noop() -> Self {
        Self { signals: None }
    }

    pub(crate) async fn recv(&mut self) -> Option<SignalEvent> {
        match &mut self.signals {
            Some(signals) => signals.recv().await,
            None => None,
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::io;
    use tokio::signal::unix::{SignalKind, signal};
    use tokio_stream::{StreamExt, StreamMap, wrappers::SignalStream};

    #[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
    enum SignalId {
        Int,
        Hup,
        Term,
        Quit,
        Tstp,
        Cont,
        Info,
        Usr1,
    }

    /// Signals for SIGINT, SIGTERM and SIGHUP on Unix.
    #[derive(Debug)]
    pub(super) struct Signals {
        // The number of streams is quite small, so a StreamMap (backed by a
        // Vec) is a good option to store the list of streams to poll.
        map: StreamMap<SignalId, SignalStream>,
        sigquit_as_info: bool,
    }

    impl Signals {
        pub(super) fn new() -> io::Result<Self> {
            let mut map = StreamMap::new();

            // Set up basic signals.
            map.extend([
                (SignalId::Int, signal_stream(SignalKind::interrupt())?),
                (SignalId::Hup, signal_stream(SignalKind::hangup())?),
                (SignalId::Term, signal_stream(SignalKind::terminate())?),
                (SignalId::Quit, signal_stream(SignalKind::quit())?),
                (SignalId::Tstp, signal_stream(tstp_kind())?),
                (SignalId::Cont, signal_stream(cont_kind())?),
                (SignalId::Usr1, signal_stream(SignalKind::user_defined1())?),
            ]);

            if let Some(info_kind) = info_kind() {
                map.insert(SignalId::Info, signal_stream(info_kind)?);
            }

            // This is a debug-only environment variable to let ctrl-\ (SIGQUIT)
            // behave like SIGINFO. Useful for testing signal-based info queries
            // on Linux.
            let sigquit_as_info =
                std::env::var("__NEXTEST_SIGQUIT_AS_INFO").is_ok_and(|v| v == "1");

            Ok(Self {
                map,
                sigquit_as_info,
            })
        }

        /// Creates a signal handler for debugger mode.
        ///
        /// SIGINT and SIGQUIT are set to SIG_IGN so nextest ignores them. The
        /// debugger will also receive these signals and will handle them as
        /// appropriate.
        ///
        /// SIGTSTP and SIGCONT are handled for internal bookkeeping (pausing/
        /// resuming timers) but are not propagated to child processes.
        pub(super) fn debugger_mode() -> io::Result<Self> {
            use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

            // Set SIGINT and SIGQUIT to SIG_IGN so nextest ignores them
            // and they only affect the debugger process.
            let ignore_action =
                SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());

            unsafe {
                let _ = sigaction(Signal::SIGINT, &ignore_action);
                let _ = sigaction(Signal::SIGQUIT, &ignore_action);
            }

            let mut map = StreamMap::new();

            // Set up termination signals and job control signals.
            // Job control signals are handled for internal bookkeeping but not
            // propagated to children.
            map.extend([
                (SignalId::Hup, signal_stream(SignalKind::hangup())?),
                (SignalId::Term, signal_stream(SignalKind::terminate())?),
                (SignalId::Tstp, signal_stream(tstp_kind())?),
                (SignalId::Cont, signal_stream(cont_kind())?),
            ]);

            Ok(Self {
                map,
                sigquit_as_info: false,
            })
        }

        pub(super) async fn recv(&mut self) -> Option<SignalEvent> {
            self.map.next().await.map(|(id, _)| match id {
                SignalId::Int => {
                    SignalEvent::Shutdown(ShutdownEvent::Signal(ShutdownSignalEvent::Interrupt))
                }
                SignalId::Hup => {
                    SignalEvent::Shutdown(ShutdownEvent::Signal(ShutdownSignalEvent::Hangup))
                }
                SignalId::Term => {
                    SignalEvent::Shutdown(ShutdownEvent::Signal(ShutdownSignalEvent::Term))
                }
                SignalId::Quit => {
                    if self.sigquit_as_info {
                        SignalEvent::Info(SignalInfoEvent::Info)
                    } else {
                        SignalEvent::Shutdown(ShutdownEvent::Signal(ShutdownSignalEvent::Quit))
                    }
                }
                SignalId::Tstp => SignalEvent::JobControl(JobControlEvent::Stop),
                SignalId::Cont => SignalEvent::JobControl(JobControlEvent::Continue),
                SignalId::Info => SignalEvent::Info(SignalInfoEvent::Info),
                SignalId::Usr1 => SignalEvent::Info(SignalInfoEvent::Usr1),
            })
        }
    }

    fn signal_stream(kind: SignalKind) -> io::Result<SignalStream> {
        Ok(SignalStream::new(signal(kind)?))
    }

    fn tstp_kind() -> SignalKind {
        SignalKind::from_raw(libc::SIGTSTP)
    }

    fn cont_kind() -> SignalKind {
        SignalKind::from_raw(libc::SIGCONT)
    }

    // The SIGINFO signal is available on many Unix platforms, but not all of
    // them.
    cfg_if::cfg_if! {
        if #[cfg(any(
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "macos",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "illumos",
        ))] {
            fn info_kind() -> Option<SignalKind> {
                Some(SignalKind::info())
            }
        } else {
            fn info_kind() -> Option<SignalKind> {
                None
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use tokio::signal::windows::{CtrlC, ctrl_c};

    #[derive(Debug)]
    pub(super) struct Signals {
        ctrl_c: CtrlC,
        ctrl_c_done: bool,
    }

    impl Signals {
        pub(super) fn new() -> std::io::Result<Self> {
            let ctrl_c = ctrl_c()?;
            Ok(Self {
                ctrl_c,
                ctrl_c_done: false,
            })
        }

        /// Creates a signal handler for debugger mode.
        /// On Windows, we don't handle Ctrl-C in debugger mode, allowing the debugger to handle it.
        pub(super) fn debugger_mode() -> std::io::Result<Self> {
            // Create a ctrl_c handler but mark it as done immediately,
            // so recv() will always return None
            let ctrl_c = ctrl_c()?;
            Ok(Self {
                ctrl_c,
                ctrl_c_done: true,
            })
        }

        pub(super) async fn recv(&mut self) -> Option<SignalEvent> {
            if self.ctrl_c_done {
                return None;
            }

            match self.ctrl_c.recv().await {
                Some(()) => Some(SignalEvent::Shutdown(ShutdownEvent::Signal(
                    ShutdownSignalEvent::Interrupt,
                ))),
                None => {
                    self.ctrl_c_done = true;
                    None
                }
            }
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum SignalEvent {
    #[cfg(unix)]
    JobControl(JobControlEvent),
    Shutdown(ShutdownEvent),
    #[cfg_attr(not(unix), expect(dead_code))]
    Info(SignalInfoEvent),
}

// A job-control related signal event.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum JobControlEvent {
    #[cfg(unix)]
    Stop,
    #[cfg(unix)]
    Continue,
}

// A signal event that should cause a shutdown to happen.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum ShutdownSignalEvent {
    #[cfg(unix)]
    Hangup,
    #[cfg(unix)]
    Term,
    #[cfg(unix)]
    Quit,
    Interrupt,
}

impl ShutdownSignalEvent {
    #[cfg(test)]
    pub(crate) const ALL_VARIANTS: &'static [Self] = &[
        #[cfg(unix)]
        Self::Hangup,
        #[cfg(unix)]
        Self::Term,
        #[cfg(unix)]
        Self::Quit,
        Self::Interrupt,
    ];
}

// An event that should cause a shutdown to happen.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum ShutdownEvent {
    /// A signal was received from the OS.
    Signal(ShutdownSignalEvent),
    /// A test failure occurred with immediate termination mode.
    TestFailureImmediate,
}

impl ShutdownEvent {
    // On Unix, send SIGTERM for termination (global timeout, test failure with immediate mode).
    #[cfg(unix)]
    pub(crate) const TERMINATE: Self = Self::Signal(ShutdownSignalEvent::Term);

    // On Windows, the best we can do is to interrupt the process.
    #[cfg(not(unix))]
    pub(crate) const TERMINATE: Self = Self::Signal(ShutdownSignalEvent::Interrupt);
}

// A signal event to query information about tests.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum SignalInfoEvent {
    /// SIGUSR1
    #[cfg(unix)]
    Usr1,

    /// SIGINFO
    #[cfg(unix)]
    Info,
}
