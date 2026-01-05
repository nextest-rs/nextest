// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pager support for nextest output.
//!
//! This module provides functionality to page output through an external pager
//! (like `less`) when appropriate. Paging is useful for commands that produce
//! long output, such as `nextest list`.

use crate::user_config::elements::{PagerSetting, PaginateSetting};
use std::{
    io::{self, IsTerminal, Stdout, Write},
    process::{Child, ChildStdin, Stdio},
};
use tracing::warn;

/// Output wrapper that optionally pages output through an external pager.
///
/// When a pager is active, output is piped to the pager process. When
/// finalized, the pager's stdin is closed and we wait for it to exit.
///
/// Implements [`Drop`] to ensure cleanup happens even if [`finalize`] is not
/// called explicitly. During a panic, stdin is closed but we skip waiting for
/// the pager to avoid potential double-panic.
///
/// [`finalize`]: Self::finalize
pub enum PagedOutput {
    /// Direct output to terminal (no paging).
    Terminal {
        /// Standard output handle.
        stdout: Stdout,
    },
    /// Output through an external pager process.
    ExternalPager {
        /// The pager child process.
        child: Child,
        /// Stdin pipe to the pager (for writing output).
        ///
        /// This is an `Option` to allow taking ownership in [`Drop`] and
        /// [`finalize`](Self::finalize).
        child_stdin: Option<ChildStdin>,
    },
}

impl PagedOutput {
    /// Creates a new terminal output (no paging).
    pub fn terminal() -> Self {
        Self::Terminal {
            stdout: io::stdout(),
        }
    }

    /// Attempts to spawn a pager if conditions are met.
    ///
    /// Returns `Terminal` output if:
    /// - `paginate` is `Never`
    /// - stdout is not a TTY
    /// - the pager command fails to spawn
    ///
    /// On pager spawn failure, a warning is logged and terminal output is
    /// returned.
    pub fn request_pager(pager: &PagerSetting, paginate: PaginateSetting) -> Self {
        // Check if paging is disabled.
        if matches!(paginate, PaginateSetting::Never) {
            return Self::terminal();
        }

        // Get the pager command.
        let PagerSetting::External(command_and_args) = pager;

        // Check if stdout is a TTY.
        if !io::stdout().is_terminal() {
            return Self::terminal();
        }

        // Try to spawn the pager.
        let mut cmd = command_and_args.to_command();
        cmd.stdin(Stdio::piped());

        match cmd.spawn() {
            Ok(mut child) => {
                let child_stdin = child
                    .stdin
                    .take()
                    .expect("child stdin should be present when piped");
                Self::ExternalPager {
                    child,
                    child_stdin: Some(child_stdin),
                }
            }
            Err(error) => {
                warn!(
                    "failed to spawn pager '{}': {error}",
                    command_and_args.command_name()
                );
                Self::terminal()
            }
        }
    }

    /// Returns a writer for stdout.
    ///
    /// For terminal output, this returns stdout directly.
    /// For paged output, this returns the pager's stdin.
    ///
    /// # Panics
    ///
    /// Panics if called after [`finalize`](Self::finalize).
    pub fn stdout(&mut self) -> &mut dyn Write {
        match self {
            Self::Terminal { stdout, .. } => stdout,
            Self::ExternalPager { child_stdin, .. } => {
                child_stdin.as_mut().expect("stdout called after finalize")
            }
        }
    }

    /// Finalizes the pager output.
    ///
    /// For terminal output, this is a no-op.
    /// For paged output, this closes the pager's stdin and waits for the pager
    /// process to exit. Errors during wait are logged but not propagated.
    ///
    /// This method is also called by [`Drop`], so explicit calls are optional
    /// but recommended for clarity.
    pub fn finalize(mut self) {
        self.finalize_inner();
    }

    fn finalize_inner(&mut self) {
        match self {
            Self::Terminal { .. } => {
                // Nothing to do.
            }
            Self::ExternalPager { child, child_stdin } => {
                // If stdin is already taken, we've already finalized.
                let Some(stdin) = child_stdin.take() else {
                    return;
                };

                // Close stdin to signal EOF to the pager.
                drop(stdin);

                // Wait for the pager to exit.
                if let Err(error) = child.wait() {
                    warn!("failed to wait on pager: {error}");
                }
                // Note: We intentionally ignore the exit status. The pager may
                // exit with a non-zero status if the user quits early (e.g.,
                // pressing 'q' in less), which is normal behavior.
            }
        }
    }
}

impl Drop for PagedOutput {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // During a panic, close stdin to signal EOF but don't wait for the
            // pager. This avoids potential issues if wait() were to panic.
            if let Self::ExternalPager { child_stdin, .. } = self {
                drop(child_stdin.take());
            }
            return;
        }
        self.finalize_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_output() {
        let mut output = PagedOutput::terminal();
        // Just verify we can get a writer.
        let _ = output.stdout();
        output.finalize();
    }

    #[test]
    fn test_terminal_output_drop() {
        // Verify Drop works without explicit finalize.
        let mut output = PagedOutput::terminal();
        let _ = output.stdout();
        // No explicit finalize - Drop handles it.
    }

    #[test]
    fn test_request_pager_never_paginate() {
        let pager = PagerSetting::default();
        let output = PagedOutput::request_pager(&pager, PaginateSetting::Never);
        assert!(matches!(output, PagedOutput::Terminal { .. }));
        output.finalize();
    }

    #[test]
    #[cfg(unix)]
    fn test_external_pager_write_and_finalize() {
        // Spawn `cat` as a simple pager that consumes input.
        let mut child = std::process::Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn cat");

        let child_stdin = child.stdin.take().expect("stdin should be piped");

        let mut output = PagedOutput::ExternalPager {
            child,
            child_stdin: Some(child_stdin),
        };

        // Write some data.
        writeln!(output.stdout(), "hello pager").expect("write should succeed");

        // Finalize should close stdin and wait for cat to exit.
        output.finalize();
    }

    #[test]
    #[cfg(unix)]
    fn test_external_pager_drop_without_finalize() {
        let mut child = std::process::Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn cat");

        let child_stdin = child.stdin.take().expect("stdin should be piped");

        let mut output = PagedOutput::ExternalPager {
            child,
            child_stdin: Some(child_stdin),
        };

        writeln!(output.stdout(), "hello pager").expect("write should succeed");

        // No explicit finalize, so Drop should handle cleanup.
        drop(output);
    }

    #[test]
    #[cfg(unix)]
    fn test_external_pager_double_finalize_is_idempotent() {
        let mut child = std::process::Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn cat");

        let child_stdin = child.stdin.take().expect("stdin should be piped");

        let mut output = PagedOutput::ExternalPager {
            child,
            child_stdin: Some(child_stdin),
        };

        // Call finalize_inner twice - second call should be a no-op.
        output.finalize_inner();
        output.finalize_inner();
        // Drop will also try to finalize, should be safe.
    }
}
