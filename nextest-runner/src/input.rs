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
use std::{
    io::IsTerminal,
    sync::{Arc, Mutex},
};
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
        if std::io::stdin().is_terminal() {
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
            debug!("not reading input because stdin is not a tty");
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
            match next {
                Ok(Event::Key(key)) => {
                    if key.code == KeyCode::Char(Self::INFO_CHAR) && key.modifiers.is_empty() {
                        return Some(InputEvent::Info);
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
            if let Err(error) = ret2.finish() {
                eprintln!(
                    "failed to restore terminal state: {}",
                    DisplayErrorChain::new(error)
                );
            }
            panic_hook(info);
        }));

        Ok(ret)
    }

    fn finish(&self) -> Result<(), InputHandlerFinishError> {
        // Do not panic here, in case a panic happened while the thread was
        // locked. Instead, ignore the error.
        let mut locked = self
            .guard
            .lock()
            .map_err(|_| InputHandlerFinishError::Poisoned)?;
        locked.finish().map_err(InputHandlerFinishError::Restore)
    }
}

// Defense in depth -- use both the Drop impl (for regular drops and
// panic=unwind) and a panic hook (for panic=abort).
impl Drop for InputHandlerImpl {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
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
    use libc::{tcgetattr, tcsetattr, ECHO, ICANON, TCSAFLUSH, TCSANOW, VMIN, VTIME};
    use std::{ffi::c_int, io, mem, os::fd::AsRawFd};

    pub(super) type Error = io::Error;

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

            stdin_tcsetattr(TCSAFLUSH, &updated)?;

            Ok(Self {
                original: Some(original),
            })
        }

        pub(super) fn finish(&mut self) -> io::Result<()> {
            if let Some(original) = self.original.take() {
                stdin_tcsetattr(TCSANOW, &original)
            } else {
                Ok(())
            }
        }
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
    use std::{io, os::windows::io::AsRawHandle};
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, SetConsoleMode, CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT,
    };

    pub(super) type Error = io::Error;

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

        pub(super) fn finish(&mut self) -> io::Result<()> {
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
}
