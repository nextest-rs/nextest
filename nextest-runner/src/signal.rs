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

    /// A no-op signal handler. Useful for tests.
    Noop,
}

impl SignalHandlerKind {
    pub(crate) fn build(self) -> Result<SignalHandler, SignalHandlerSetupError> {
        match self {
            Self::Standard => SignalHandler::new(),
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
    use tokio::signal::unix::{signal, Signal, SignalKind};

    /// Signals for SIGINT, SIGTERM and SIGHUP on Unix.
    #[derive(Debug)]
    pub(super) struct Signals {
        sigint: SignalWithDone,
        sighup: SignalWithDone,
        sigterm: SignalWithDone,
        sigtstp: SignalWithDone,
        sigcont: SignalWithDone,
    }

    impl Signals {
        pub(super) fn new() -> std::io::Result<Self> {
            let sigint = SignalWithDone::new(SignalKind::interrupt())?;
            let sighup = SignalWithDone::new(SignalKind::hangup())?;
            let sigterm = SignalWithDone::new(SignalKind::terminate())?;
            let sigtstp = SignalWithDone::new(SignalKind::from_raw(libc::SIGTSTP))?;
            let sigcont = SignalWithDone::new(SignalKind::from_raw(libc::SIGCONT))?;

            Ok(Self {
                sigint,
                sighup,
                sigterm,
                sigtstp,
                sigcont,
            })
        }

        pub(super) async fn recv(&mut self) -> Option<SignalEvent> {
            loop {
                tokio::select! {
                    recv = self.sigint.signal.recv(), if !self.sigint.done => {
                        match recv {
                            Some(()) => break Some(SignalEvent::Shutdown(ShutdownEvent::Interrupt)),
                            None => self.sigint.done = true,
                        }
                    }
                    recv = self.sighup.signal.recv(), if !self.sighup.done => {
                        match recv {
                            Some(()) => break Some(SignalEvent::Shutdown(ShutdownEvent::Hangup)),
                            None => self.sighup.done = true,
                        }
                    }
                    recv = self.sigterm.signal.recv(), if !self.sigterm.done => {
                        match recv {
                            Some(()) => break Some(SignalEvent::Shutdown(ShutdownEvent::Term)),
                            None => self.sigterm.done = true,
                        }
                    }
                    recv = self.sigtstp.signal.recv(), if !self.sigtstp.done => {
                        match recv {
                            Some(()) => break Some(SignalEvent::JobControl(JobControlEvent::Stop)),
                            None => self.sigtstp.done = true,
                        }
                    }
                    recv = self.sigcont.signal.recv(), if !self.sigcont.done => {
                        match recv {
                            Some(()) => break Some(SignalEvent::JobControl(JobControlEvent::Continue)),
                            None => self.sigcont.done = true,
                        }
                    }
                    else => {
                        break None
                    }
                }
            }
        }
    }

    #[derive(Debug)]
    struct SignalWithDone {
        signal: Signal,
        done: bool,
    }

    impl SignalWithDone {
        fn new(kind: SignalKind) -> std::io::Result<Self> {
            let signal = signal(kind)?;
            Ok(Self {
                signal,
                done: false,
            })
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use tokio::signal::windows::{ctrl_c, CtrlC};

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

        pub(super) async fn recv(&mut self) -> Option<SignalEvent> {
            if self.ctrl_c_done {
                return None;
            }

            match self.ctrl_c.recv().await {
                Some(()) => Some(SignalEvent::Shutdown(ShutdownEvent::Interrupt)),
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
pub(crate) enum ShutdownEvent {
    #[cfg(unix)]
    Hangup,
    #[cfg(unix)]
    Term,
    Interrupt,
}
