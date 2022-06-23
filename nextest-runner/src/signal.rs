// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for handling signals in nextest.

use crate::errors::SignalHandlerSetupError;
use crossbeam_channel::Receiver;

/// A receiver that generates signals if ctrl-c is pressed.
///
/// A `SignalHandler` can be passed into
/// [`TestRunnerBuilder::build`](crate::runner::TestRunnerBuilder::build).
#[derive(Debug)]
pub struct SignalHandler {
    pub(crate) receiver: Receiver<SignalEvent>,
}

impl SignalHandler {
    /// Creates a new `SignalReceiver` that handles Ctrl-C errors.
    ///
    /// Errors if a signal handler has already been registered in this process. Only one signal
    /// handler can be registered for a process at any given time.
    pub fn new() -> Result<Self, SignalHandlerSetupError> {
        let (sender, receiver) = crossbeam_channel::unbounded();
        ctrlc::set_handler(move || {
            let _ = sender.send(SignalEvent::Interrupted);
        })?;

        Ok(Self { receiver })
    }

    /// Creates a new `SignalReceiver` that does nothing.
    pub fn noop() -> Self {
        let (_sender, receiver) = crossbeam_channel::bounded(1);
        Self { receiver }
    }
}

// Just a single-valued enum for now, might have more information in the future.
#[derive(Debug)]
pub(crate) enum SignalEvent {
    Interrupted,
}
