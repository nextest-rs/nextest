// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Input handling for nextest.
//!
//! Similar to signal handling, input handling is read by the runner and used to control
//! non-signal-related aspects of the test run. For example, "i" to print information about which
//! tests are currently running.

use crate::errors::DisplayErrorChain;
use crossterm::event::{Event, EventStream, KeyCode};
use futures::StreamExt;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tracing::{debug, warn};

/// The kind of input handling to set up for a test run.
///
/// An `InputHandlerKind` can be passed into
/// [`TestRunnerBuilder::build`](crate::runner::TestRunnerBuilder::build).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputHandlerKind {
    /// The standard input handler, which reads from standard input.
    Standard,

    /// A no-op input handler. Useful for tests.
    Noop,
}

impl InputHandlerKind {
    pub(crate) fn build(self) -> InputHandler {
        match self {
            Self::Standard => InputHandler::new(),
            Self::Noop => InputHandler::noop(),
        }
    }
}

/// The input handler implementation.
#[derive(Debug)]
pub(crate) struct InputHandler {
    // A scope guard that ensures non-canonical mode is disabled when this is
    // dropped, along with a stream to read events from.
    imp: Option<(InputHandlerImpl, EventStream)>,
}

impl InputHandler {
    const INFO_CHAR: char = 't';

    /// Creates a new `InputHandler` that reads from standard input.
    pub(crate) fn new() -> Self {
        if imp::is_foreground_process() {
            // Try enabling non-canonical mode.
            match InputHandlerImpl::new() {
                Ok(handler) => {
                    let stream = EventStream::new();
                    debug!("enabled terminal non-canonical mode, reading input events");
                    Self {
                        imp: Some((handler, stream)),
                    }
                }
                Err(error) => {
                    warn!(
                        "failed to enable terminal non-canonical mode, \
                         cannot read input events: {}",
                        error,
                    );
                    Self::noop()
                }
            }
        } else {
            debug!(
                "not reading input because nextest is not \
                 a foreground process in a terminal"
            );
            Self::noop()
        }
    }

    /// Creates a new `InputHandler` that does nothing.
    pub(crate) fn noop() -> Self {
        Self { imp: None }
    }

    pub(crate) fn status(&self) -> InputHandlerStatus {
        if self.imp.is_some() {
            InputHandlerStatus::Enabled {
                info_char: Self::INFO_CHAR,
            }
        } else {
            InputHandlerStatus::Disabled
        }
    }

    /// Receives an event from the input, or None if the input is closed and there are no more
    /// events.
    ///
    /// This is a cancel-safe operation.
    pub(crate) async fn recv(&mut self) -> Option<InputEvent> {
        let (_, stream) = self.imp.as_mut()?;
        loop {
            let next = stream.next().await?;
            // Everything after here must be cancel-safe: ideally no await
            // points at all, but okay with discarding `next` if there are any
            // await points.
            match next {
                Ok(Event::Key(key)) => {
                    if key.code == KeyCode::Char(Self::INFO_CHAR) && key.modifiers.is_empty() {
                        return Some(InputEvent::Info);
                    }
                    if key.code == KeyCode::Enter {
                        return Some(InputEvent::Enter);
                    }
                }
                Ok(event) => {
                    debug!("unhandled event: {:?}", event);
                }
                Err(error) => {
                    warn!("failed to read input event: {}", error);
                }
            }
        }
    }

    /// Suspends the input handler temporarily, restoring the original terminal
    /// state.
    ///
    /// Used by the stop signal handler.
    #[cfg(unix)]
    pub(crate) fn suspend(&mut self) {
        let Some((handler, _)) = self.imp.as_mut() else {
            return;
        };

        if let Err(error) = handler.restore() {
            warn!("failed to suspend terminal non-canonical mode: {}", error);
            // Don't set imp to None -- we want to try to reinit() on resume.
        }
    }

    /// Resumes the input handler after a suspension.
    ///
    /// Used by the continue signal handler.
    #[cfg(unix)]
    pub(crate) fn resume(&mut self) {
        let Some((handler, _)) = self.imp.as_mut() else {
            // None means that the input handler is disabled, so there is
            // nothing to resume.
            return;
        };

        if let Err(error) = handler.reinit() {
            warn!(
                "failed to resume terminal non-canonical mode, \
                 cannot read input events: {}",
                error
            );
            // Do set self.imp to None in this case -- we want to indicate to
            // callers (e.g. via status()) that the input handler is disabled.
            self.imp = None;
        }
    }
}

/// The status of the input handler, returned by
/// [`TestRunner::input_handler_status`](crate::runner::TestRunner::input_handler_status).
pub enum InputHandlerStatus {
    /// The input handler is enabled.
    Enabled {
        /// The character that triggers the "info" event.
        info_char: char,
    },

    /// The input handler is disabled.
    Disabled,
}

#[derive(Clone, Debug)]
struct InputHandlerImpl {
    // `Arc<Mutex<_>>` for coordination between the drop handler and the panic
    // hook.
    guard: Arc<Mutex<imp::InputGuard>>,
}

impl InputHandlerImpl {
    fn new() -> Result<Self, InputHandlerCreateError> {
        let guard = imp::InputGuard::new().map_err(InputHandlerCreateError::EnableNonCanonical)?;

        // At this point, the new terminal state is committed. Install a
        // panic hook to restore the original state.
        let ret = Self {
            guard: Arc::new(Mutex::new(guard)),
        };

        let ret2 = ret.clone();
        let panic_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Ignore errors to avoid double-panicking.
            if let Err(error) = ret2.restore() {
                eprintln!(
                    "failed to restore terminal state: {}",
                    DisplayErrorChain::new(error)
                );
            }
            panic_hook(info);
        }));

        Ok(ret)
    }

    #[cfg(unix)]
    fn reinit(&self) -> Result<(), InputHandlerCreateError> {
        // Make a new input guard and replace the old one. Don't set a new panic
        // hook.
        //
        // The mutex is shared by the panic hook and self/the drop handler, so
        // the change below will also be visible to the panic hook. But we
        // acquire the mutex first to avoid a potential race where multiple
        // calls to reinit() can happen concurrently.
        //
        // Also note that if this fails, the old InputGuard will be visible to
        // the panic hook, which is fine -- since we called restore() first, the
        // terminal state is already restored and guard is None.
        let mut locked = self
            .guard
            .lock()
            .map_err(|_| InputHandlerCreateError::Poisoned)?;
        let guard = imp::InputGuard::new().map_err(InputHandlerCreateError::EnableNonCanonical)?;
        *locked = guard;
        Ok(())
    }

    fn restore(&self) -> Result<(), InputHandlerFinishError> {
        // Do not panic here, in case a panic happened while the thread was
        // locked. Instead, ignore the error.
        let mut locked = self
            .guard
            .lock()
            .map_err(|_| InputHandlerFinishError::Poisoned)?;
        locked.restore().map_err(InputHandlerFinishError::Restore)
    }
}

// Defense in depth -- use both the Drop impl (for regular drops and
// panic=unwind) and a panic hook (for panic=abort).
impl Drop for InputHandlerImpl {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!(
                "failed to restore terminal state: {}",
                DisplayErrorChain::new(error)
            );
        }
    }
}

#[derive(Debug, Error)]
enum InputHandlerCreateError {
    #[error("failed to enable terminal non-canonical mode")]
    EnableNonCanonical(#[source] imp::Error),

    #[cfg(unix)]
    #[error("mutex was poisoned while reinitializing terminal state")]
    Poisoned,
}

#[derive(Debug, Error)]
enum InputHandlerFinishError {
    #[error("mutex was poisoned while restoring terminal state")]
    Poisoned,

    #[error("failed to restore terminal state")]
    Restore(#[source] imp::Error),
}

#[cfg(unix)]
mod imp {
    use libc::{ECHO, ICANON, TCSAFLUSH, TCSANOW, VMIN, VTIME, tcgetattr, tcsetattr};
    use std::{
        ffi::c_int,
        io::{self, IsTerminal},
        mem,
        os::fd::AsRawFd,
    };
    use tracing::debug;

    pub(super) type Error = io::Error;

    pub(super) fn is_foreground_process() -> bool {
        if !std::io::stdin().is_terminal() {
            debug!("stdin is not a terminal => is_foreground_process() is false");
            return false;
        }

        // Also check that tcgetpgrp is the same. If tcgetpgrp fails, it'll
        // return -1 and this check will fail.
        //
        // See https://stackoverflow.com/a/2428429.
        let pgrp = unsafe { libc::getpgrp() };
        let tc_pgrp = unsafe { libc::tcgetpgrp(std::io::stdin().as_raw_fd()) };
        if tc_pgrp == -1 {
            debug!(
                "stdin is a terminal, and pgrp = {pgrp}, but tcgetpgrp failed with error {} => \
                 is_foreground_process() is false",
                io::Error::last_os_error()
            );
            return false;
        }
        if pgrp != tc_pgrp {
            debug!(
                "stdin is a terminal, but pgrp {} != tcgetpgrp {} => is_foreground_process() is false",
                pgrp, tc_pgrp
            );
            return false;
        }

        debug!(
            "stdin is a terminal, and pgrp {pgrp} == tcgetpgrp {tc_pgrp} => \
             is_foreground_process() is true"
        );
        true
    }

    /// A scope guard to enable non-canonical input mode on Unix platforms.
    ///
    /// Importantly, this does not enable the full raw mode that crossterm
    /// provides -- that disables things like signal processing via the terminal
    /// driver, which is unnecessary for our purposes. Here we only disable
    /// options relevant to the input: echoing and canonical mode.
    #[derive(Clone, Debug)]
    pub(super) struct InputGuard {
        // None indicates that the original state has been restored -- only one
        // entity should do this.
        //
        // Note: originally, this used nix's termios support, but that was found
        // to be buggy on illumos (lock up the terminal) -- apparently, not all
        // bitflags were modeled. Using libc directly is more reliable.
        original: Option<libc::termios>,
    }

    impl InputGuard {
        pub(super) fn new() -> io::Result<Self> {
            let TermiosPair { original, updated } = compute_termios()?;
            stdin_tcsetattr(TCSAFLUSH, &updated)?;

            // Ignore SIGTTIN and SIGTTOU while input handling is active. This
            // prevents the process from being stopped if a test spawns an
            // interactive shell that takes over the foreground process group.
            //
            // This is what zsh does for job control:
            // https://github.com/zsh-users/zsh/blob/3e72a52/Src/init.c#L1439
            //
            // See https://github.com/nextest-rs/nextest/issues/2878.
            unsafe {
                libc::signal(libc::SIGTTIN, libc::SIG_IGN);
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            }

            Ok(Self {
                original: Some(original),
            })
        }

        pub(super) fn restore(&mut self) -> io::Result<()> {
            if let Some(original) = self.original.take() {
                // Restore terminal state. SIGTTIN/SIGTTOU remain ignored for
                // the process lifetime to avoid racing with crossterm's input
                // thread shutdown.
                stdin_tcsetattr(TCSANOW, &original)
            } else {
                Ok(())
            }
        }
    }

    fn compute_termios() -> io::Result<TermiosPair> {
        let mut termios = mem::MaybeUninit::uninit();
        let res = unsafe { tcgetattr(std::io::stdin().as_raw_fd(), termios.as_mut_ptr()) };
        if res == -1 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: if res is 0, then termios has been initialized.
        let original = unsafe { termios.assume_init() };

        let mut updated = original;

        // Disable echoing inputs and canonical mode. We don't disable things like ISIG -- we
        // handle that via the signal handler.
        updated.c_lflag &= !(ECHO | ICANON);
        // VMIN is 1 and VTIME is 0: this enables blocking reads of 1 byte
        // at a time with no timeout. See
        // https://linux.die.net/man/3/tcgetattr's "Canonical and
        // noncanonical mode" section.
        updated.c_cc[VMIN] = 1;
        updated.c_cc[VTIME] = 0;

        Ok(TermiosPair { original, updated })
    }

    #[derive(Clone, Debug)]
    struct TermiosPair {
        original: libc::termios,
        updated: libc::termios,
    }

    fn stdin_tcsetattr(optional_actions: c_int, updated: &libc::termios) -> io::Result<()> {
        let res = unsafe { tcsetattr(std::io::stdin().as_raw_fd(), optional_actions, updated) };
        if res == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::{
        io::{self, IsTerminal},
        os::windows::io::AsRawHandle,
    };
    use tracing::debug;
    use windows_sys::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, GetConsoleMode, SetConsoleMode,
    };

    pub(super) type Error = io::Error;

    pub(super) fn is_foreground_process() -> bool {
        // Windows doesn't have a notion of foreground and background process
        // groups: https://github.com/microsoft/terminal/issues/680. So simply
        // checking that stdin is a terminal is enough.
        //
        // This function is written slightly non-idiomatically, because it
        // follows the same structure as the more complex Unix function above.
        if !std::io::stdin().is_terminal() {
            debug!("stdin is not a terminal => is_foreground_process() is false");
            return false;
        }

        debug!("stdin is a terminal => is_foreground_process() is true");
        true
    }

    /// A scope guard to enable raw input mode on Windows.
    ///
    /// Importantly, this does not mask out `ENABLE_PROCESSED_INPUT` like
    /// crossterm does -- that disables things like signal processing via the
    /// terminal driver, which is unnecessary for our purposes. Here we only
    /// disable options relevant to the input: `ENABLE_LINE_INPUT` and
    /// `ENABLE_ECHO_INPUT`.
    #[derive(Clone, Debug)]
    pub(super) struct InputGuard {
        original: Option<CONSOLE_MODE>,
    }

    impl InputGuard {
        pub(super) fn new() -> io::Result<Self> {
            let handle = std::io::stdin().as_raw_handle();

            // Read the original console mode.
            let mut original: CONSOLE_MODE = 0;
            let res = unsafe { GetConsoleMode(handle, &mut original) };
            if res == 0 {
                return Err(io::Error::last_os_error());
            }

            // Mask out ENABLE_LINE_INPUT and ENABLE_ECHO_INPUT.
            let updated = original & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT);

            // Set the new console mode.
            let res = unsafe { SetConsoleMode(handle, updated) };
            if res == 0 {
                return Err(io::Error::last_os_error());
            }

            Ok(Self {
                original: Some(original),
            })
        }

        pub(super) fn restore(&mut self) -> io::Result<()> {
            if let Some(original) = self.original.take() {
                let handle = std::io::stdin().as_raw_handle();
                let res = unsafe { SetConsoleMode(handle, original) };
                if res == 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            }
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum InputEvent {
    Info,
    Enter,
}
