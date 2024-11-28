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
    use std::io;
    use tokio::signal::unix::{signal, SignalKind};
    use tokio_stream::{wrappers::SignalStream, StreamExt, StreamMap};

    #[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
    enum SignalId {
        Int,
        Hup,
        Term,
        Quit,
        Tstp,
        Cont,
    }

    /// Signals for SIGINT, SIGTERM and SIGHUP on Unix.
    #[derive(Debug)]
    pub(super) struct Signals {
        // The number of streams is quite small, so a StreamMap (backed by a
        // Vec) is a good option to store the list of streams to poll.
        map: StreamMap<SignalId, SignalStream>,
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
            ]);

            Ok(Self { map })
        }

        pub(super) async fn recv(&mut self) -> Option<SignalEvent> {
            self.map.next().await.map(|(id, _)| match id {
                SignalId::Int => SignalEvent::Shutdown(ShutdownEvent::Interrupt),
                SignalId::Hup => SignalEvent::Shutdown(ShutdownEvent::Hangup),
                SignalId::Term => SignalEvent::Shutdown(ShutdownEvent::Term),
                SignalId::Quit => SignalEvent::Shutdown(ShutdownEvent::Quit),
                SignalId::Tstp => SignalEvent::JobControl(JobControlEvent::Stop),
                SignalId::Cont => SignalEvent::JobControl(JobControlEvent::Continue),
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
    #[cfg(unix)]
    Quit,
    Interrupt,
}
