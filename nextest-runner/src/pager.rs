// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pager support for nextest output.
//!
//! This module provides functionality to page output through an external pager
//! (like `less`) or a builtin pager (streampager) when appropriate. Paging is
//! useful for commands that produce long output, such as `nextest list`.

use crate::{
    user_config::elements::{PagerSetting, PaginateSetting, StreampagerConfig},
    write_str::WriteStr,
};
use std::{
    io::{self, IsTerminal, PipeWriter, Stdout, Write},
    process::{Child, ChildStdin, Stdio},
    thread::{self, JoinHandle},
};
use tracing::warn;

/// Output wrapper that optionally pages output through a pager.
///
/// When a pager is active, output is piped to the pager process (external) or
/// thread (builtin). When finalized, the pipe is closed and we wait for the
/// pager to exit.
///
/// Implements [`Drop`] to ensure cleanup happens even if `finalize` is not
/// called explicitly. During a panic, pipes are closed but we skip waiting for
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
    /// Output through the builtin streampager.
    BuiltinPager {
        /// Pipe writer for stdout (for writing output).
        ///
        /// This is an `Option` to allow taking ownership in [`Drop`] and
        /// [`finalize`](Self::finalize).
        out_writer: Option<PipeWriter>,
        /// The pager thread handle.
        ///
        /// This is an `Option` to allow taking ownership in `finalize`.
        pager_thread: Option<JoinHandle<streampager::Result<()>>>,
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
    pub fn request_pager(
        pager: &PagerSetting,
        paginate: PaginateSetting,
        streampager_config: &StreampagerConfig,
    ) -> Self {
        // Check if paging is disabled.
        if matches!(paginate, PaginateSetting::Never) {
            return Self::terminal();
        }

        // Check if stdout is a TTY.
        if !io::stdout().is_terminal() {
            return Self::terminal();
        }

        match pager {
            PagerSetting::Builtin => Self::spawn_builtin_pager(streampager_config),
            PagerSetting::External(command_and_args) => {
                // Try to spawn the external pager.
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
        }
    }

    /// Spawns the builtin streampager.
    fn spawn_builtin_pager(config: &StreampagerConfig) -> Self {
        let streampager_config = streampager::config::Config {
            wrapping_mode: config.streampager_wrapping_mode(),
            interface_mode: config.streampager_interface_mode(),
            show_ruler: config.show_ruler,
            // Don't scroll past EOF - it can leave empty lines on screen after
            // exiting with quit-if-one-page mode.
            scroll_past_eof: false,
            ..Default::default()
        };

        // Initialize with tty instead of stdin/stdout. We spawn pager so long
        // as stdout is a tty, which means stdin may be redirected.
        let pager_result = streampager::Pager::new_using_system_terminal_with_config(
            streampager_config,
        )
        .and_then(|mut pager| {
            // Create a pipe for stdout.
            let (out_reader, out_writer) = io::pipe()?;
            pager.add_stream(out_reader, "")?;
            Ok((pager, out_writer))
        });

        match pager_result {
            Ok((pager, out_writer)) => Self::BuiltinPager {
                out_writer: Some(out_writer),
                pager_thread: Some(thread::spawn(|| pager.run())),
            },
            Err(error) => {
                warn!("failed to set up builtin pager: {error}");
                Self::terminal()
            }
        }
    }

    /// Returns true if output will be displayed interactively.
    ///
    /// This is used to determine whether to use human-readable formatting
    /// (interactive) or machine-friendly oneline formatting (piped).
    ///
    /// - For terminal output: returns whether stdout is a TTY.
    /// - For paged output: always returns true, since the pager displays
    ///   output interactively to the user.
    pub fn is_interactive(&self) -> bool {
        match self {
            Self::Terminal { stdout, .. } => stdout.is_terminal(),
            // Paged output is always interactive - the user sees it in a
            // terminal via the pager.
            Self::ExternalPager { .. } | Self::BuiltinPager { .. } => true,
        }
    }

    /// Finalizes the pager output.
    ///
    /// For terminal output, this is a no-op.
    /// For paged output, this closes the pipe and waits for the pager
    /// process/thread to exit. Errors during wait are logged but not propagated.
    ///
    /// This method is also called by [`Drop`], so explicit calls are optional
    /// but recommended for clarity.
    pub fn finalize(mut self) {
        self.finalize_inner();
    }

    // ---
    // Helper methods
    // ---

    // This is not made public: we want everyone to go through WriteStr, which
    // squelches BrokenPipe errors.
    fn stdout(&mut self) -> &mut dyn Write {
        match self {
            Self::Terminal { stdout, .. } => stdout,
            Self::ExternalPager { child_stdin, .. } => child_stdin
                .as_mut()
                .expect("stdout should not be called after finalize"),
            Self::BuiltinPager { out_writer, .. } => out_writer
                .as_mut()
                .expect("stdout should not be called after finalize"),
        }
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

                // Wait for the pager to exit. (Ignore broken pipes -- they're
                // expected with less.)
                if let Err(error) = child.wait()
                    && error.kind() != io::ErrorKind::BrokenPipe
                {
                    warn!("failed to wait on pager: {error}");
                }
                // Note: We intentionally ignore the exit status from the child process. The pager may
                // exit with a non-zero status if the user quits early (e.g.,
                // pressing 'q' in less), which is normal behavior.
            }
            Self::BuiltinPager {
                out_writer,
                pager_thread,
            } => {
                // If writer is already taken, we've already finalized.
                let Some(writer) = out_writer.take() else {
                    return;
                };

                // Close the pipe to signal EOF to the pager.
                drop(writer);

                // Wait for the pager thread to exit.
                if let Some(thread) = pager_thread.take() {
                    match thread.join() {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            warn!("failed to run builtin pager: {error}");
                        }
                        Err(_) => {
                            warn!("builtin pager thread panicked");
                        }
                    }
                }
            }
        }
    }
}

impl Drop for PagedOutput {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // During a panic, close pipes to signal EOF but don't wait for the
            // pager. This avoids potential issues if wait()/join() were to panic.
            match self {
                Self::Terminal { .. } => {}
                Self::ExternalPager { child_stdin, .. } => {
                    drop(child_stdin.take());
                }
                Self::BuiltinPager { out_writer, .. } => {
                    drop(out_writer.take());
                }
            }
            return;
        }
        self.finalize_inner();
    }
}

impl WriteStr for PagedOutput {
    fn write_str(&mut self, s: &str) -> io::Result<()> {
        squelch_broken_pipe(self.stdout().write_all(s.as_bytes()))
    }

    fn write_str_flush(&mut self) -> io::Result<()> {
        squelch_broken_pipe(self.stdout().flush())
    }
}

fn squelch_broken_pipe(res: io::Result<()>) -> io::Result<()> {
    match res {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_config::elements::{StreampagerInterface, StreampagerWrapping};

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
        let streampager = StreampagerConfig {
            interface: StreampagerInterface::QuitIfOnePage,
            wrapping: StreampagerWrapping::Word,
            show_ruler: true,
        };
        let output = PagedOutput::request_pager(&pager, PaginateSetting::Never, &streampager);
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

    #[test]
    #[cfg(unix)]
    fn test_external_pager_early_exit_squelches_broken_pipe() {
        // `true` exits immediately, causing writes to fail with BrokenPipe.
        let mut child = std::process::Command::new("true")
            .stdin(Stdio::piped())
            .spawn()
            .expect("failed to spawn true");

        let child_stdin = child.stdin.take().expect("stdin should be piped");

        // Wait for the process to exit before constructing PagedOutput.
        let _ = child.wait();

        let mut output = PagedOutput::ExternalPager {
            child,
            child_stdin: Some(child_stdin),
        };

        output
            .write_str("hello\n")
            .expect("BrokenPipe should be squelched for write_str");
        let error = output
            .stdout()
            .write(b"hello\n")
            .expect_err("Write should fail with BrokenPipe");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        output
            .write_str_flush()
            .expect("BrokenPipe should be squelched for write_str_flush");
        output.finalize();
    }
}
