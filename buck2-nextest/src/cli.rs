// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Command-line interface for `buck2-nextest`.

use crate::{
    errors::{ExpectedError, Result},
    executor::{self, ExecutorOptions, SocketSpec},
};
use camino::Utf8PathBuf;
use clap::{Args, Parser, ValueEnum};
use nextest_runner::{test_filter::RunIgnored, write_str::WriteStr};
use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::fd::RawFd;

/// A next-generation test runner for Buck2.
///
/// With no subcommand, this runs as Buck2's test executor: `buck2 test`
/// launches it with a pair of sockets and speaks gRPC over them. Point Buck2 at
/// it with `v2_test_executor` in the `[test]` section of `.buckconfig`.
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub struct App {
    /// The spec-file subcommands, for working on `buck2-nextest` itself.
    ///
    /// See [`crate::spec_cli`] for what they are and why they are not in
    /// release builds.
    #[cfg(feature = "spec-file")]
    #[command(subcommand)]
    command: Option<crate::spec_cli::Command>,

    #[command(flatten)]
    executor: ExecutorArgs,
}

/// The arguments Buck2 passes when it launches a test executor.
///
/// All of these are hidden: nobody types them, and half of them are file
/// descriptor numbers that only mean anything in the process Buck2 spawned.
#[derive(Debug, Args)]
struct ExecutorArgs {
    /// The descriptor for the socket Buck2 sends test targets on.
    #[cfg(unix)]
    #[arg(long, hide = true, value_name = "FD")]
    executor_fd: Option<RawFd>,

    /// The descriptor for the socket results are reported on.
    #[cfg(unix)]
    #[arg(long, hide = true, value_name = "FD")]
    orchestrator_fd: Option<RawFd>,

    /// The address Buck2 sends test targets on.
    #[arg(long, hide = true, value_name = "ADDR")]
    executor_addr: Option<String>,

    /// The address results are reported on.
    #[arg(long, hide = true, value_name = "ADDR")]
    orchestrator_addr: Option<String>,

    /// Buck2's trace ID for this invocation. Accepted and ignored.
    #[arg(long, hide = true, value_name = "ID")]
    buck_trace_id: Option<String>,

    /// Buck2's configuration entries, e.g. `host=linux`. Accepted and ignored.
    #[arg(long, hide = true, value_name = "KEY=VALUE")]
    config_entry: Vec<String>,

    /// The Buck2 project root.
    #[arg(long, hide = true, value_name = "PATH", default_value = ".")]
    project_root: Utf8PathBuf,

    /// Everything after `--`, i.e. `buck2 test ... -- <these>`.
    #[arg(last = true, value_name = "ARGS")]
    passthrough: Vec<String>,
}

/// The options given after `--` on a `buck2 test` command line.
///
/// Parsed separately from the executor's own arguments, since Buck2 hands them
/// over as an opaque list.
///
/// Buck2 builds that list as `["ignored", "--buck-test-info", "ignored"]`
/// followed by whatever the user wrote, where the leading entry stands in for a
/// program name (see `app/buck2_test/src/command.rs`). So this is parsed the
/// ordinary way, with the first entry consumed as the binary name, rather than
/// with `no_binary_name`.
#[derive(Debug, Parser)]
#[command(name = "buck2 test -- ")]
struct PassthroughArgs {
    /// Path to a Buck1-style test info file.
    ///
    /// Buck2 passes this on every invocation for compatibility with older
    /// external runners. It describes the same targets that arrive over gRPC,
    /// so it is accepted and ignored.
    #[arg(long, hide = true, value_name = "PATH")]
    buck_test_info: Option<Utf8PathBuf>,

    /// Extra environment variables for test processes, as `NAME=VALUE`.
    ///
    /// `buck2 test`'s own help documents this as the way to pass a variable
    /// through to tests, so it works the same way here.
    #[arg(long, value_name = "NAME=VALUE")]
    env: Vec<String>,

    #[command(flatten)]
    spec: ProfileOpts,

    #[command(flatten)]
    filter: FilterOpts,
}

impl PassthroughArgs {
    /// Splits the `--env` arguments into name and value.
    fn env(&self) -> Result<BTreeMap<String, String>> {
        self.env
            .iter()
            .map(|entry| {
                entry
                    .split_once('=')
                    .map(|(name, value)| (name.to_owned(), value.to_owned()))
                    .ok_or_else(|| ExpectedError::MalformedEnvArg { arg: entry.clone() })
            })
            .collect()
    }
}

/// The profile-selection options, which both modes share.
#[derive(Debug, Default, Args)]
struct ProfileOpts {
    /// The nextest profile to use.
    #[arg(long, short = 'P', env = "NEXTEST_PROFILE", value_name = "NAME")]
    profile: Option<String>,

    /// Path to a nextest configuration file.
    #[arg(long, value_name = "PATH")]
    config_file: Option<Utf8PathBuf>,
}

/// Options shared by every way of selecting tests.
///
/// Public because Buck2 passes these after `--`, so the executor parses them
/// from a separate argument vector rather than from the top-level command line.
#[derive(Debug, Default, Args)]
pub struct FilterOpts {
    /// Run tests matching this filterset.
    #[arg(long, short = 'E', value_name = "EXPR")]
    filterset: Vec<String>,

    /// Run ignored tests.
    #[arg(long, value_enum, value_name = "WHICH")]
    run_ignored: Option<RunIgnoredOpt>,

    /// Number of threads to list tests with.
    #[arg(long, short = 'j', value_name = "N")]
    list_threads: Option<usize>,

    /// Test name substring filters.
    #[arg(value_name = "FILTERS")]
    filters: Vec<String>,
}

impl FilterOpts {
    /// Returns the filtersets, in the order they were given.
    pub fn filtersets(&self) -> Vec<String> {
        self.filterset.clone()
    }

    /// Returns the substring filters.
    pub fn patterns(&self) -> Vec<String> {
        self.filters.clone()
    }

    /// Returns whether to run ignored tests.
    pub fn run_ignored(&self) -> RunIgnored {
        self.run_ignored.map(Into::into).unwrap_or_default()
    }

    /// Returns how many threads to list tests with.
    ///
    /// Defaults to the machine's parallelism, as `cargo-nextest` does.
    pub fn list_threads(&self) -> usize {
        self.list_threads
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RunIgnoredOpt {
    /// Run non-ignored tests.
    Default,
    /// Run ignored tests only.
    Only,
    /// Run both ignored and non-ignored tests.
    All,
}

impl From<RunIgnoredOpt> for RunIgnored {
    fn from(opt: RunIgnoredOpt) -> Self {
        match opt {
            RunIgnoredOpt::Default => Self::Default,
            RunIgnoredOpt::Only => Self::Only,
            RunIgnoredOpt::All => Self::All,
        }
    }
}

impl App {
    /// Runs the requested command, returning a process exit code.
    pub fn exec(self, writer: &mut dyn WriteStr, cli_args: Vec<String>) -> Result<i32> {
        #[cfg(feature = "spec-file")]
        if let Some(command) = self.command {
            return command.exec(writer, cli_args);
        }

        // Without a subcommand, Buck2 launched us.
        let _ = writer;
        self.executor.exec(cli_args)
    }
}

impl ExecutorArgs {
    fn exec(self, cli_args: Vec<String>) -> Result<i32> {
        let passthrough = PassthroughArgs::try_parse_from(&self.passthrough)
            .map_err(|error| ExpectedError::PassthroughParseError { error })?;

        let extra_env = passthrough.env()?;
        let (executor, orchestrator) = self.sockets()?;
        executor::exec(
            executor,
            orchestrator,
            ExecutorOptions {
                project_root: self.project_root,
                profile_name: passthrough.spec.profile,
                config_file: passthrough.spec.config_file,
                filter: passthrough.filter,
                extra_env,
                cli_args,
            },
        )
    }

    /// Works out how to reach the two sockets Buck2 set up.
    ///
    /// Buck2 passes descriptors on Unix and addresses elsewhere, and always
    /// passes both halves of a pair. Anything else means this was not launched
    /// by Buck2. These stay as plain data here: turning one into a socket has
    /// to happen inside the runtime that will drive it.
    fn sockets(&self) -> Result<(SocketSpec, SocketSpec)> {
        #[cfg(unix)]
        if let (Some(executor), Some(orchestrator)) = (self.executor_fd, self.orchestrator_fd) {
            return Ok((SocketSpec::Fd(executor), SocketSpec::Fd(orchestrator)));
        }

        if let (Some(executor), Some(orchestrator)) = (&self.executor_addr, &self.orchestrator_addr)
        {
            return Ok((
                SocketSpec::Addr(executor.clone()),
                SocketSpec::Addr(orchestrator.clone()),
            ));
        }

        Err(self.socket_error())
    }

    /// Describes what was missing, so a half-configured launch says so.
    fn socket_error(&self) -> ExpectedError {
        #[cfg(unix)]
        {
            if self.executor_fd.is_some() && self.orchestrator_fd.is_none() {
                return ExpectedError::IncompleteExecutorSockets {
                    given: "executor",
                    missing: "orchestrator",
                };
            }
            if self.orchestrator_fd.is_some() && self.executor_fd.is_none() {
                return ExpectedError::IncompleteExecutorSockets {
                    given: "orchestrator",
                    missing: "executor",
                };
            }
        }
        if self.executor_addr.is_some() && self.orchestrator_addr.is_none() {
            return ExpectedError::IncompleteExecutorSockets {
                given: "executor",
                missing: "orchestrator",
            };
        }
        if self.orchestrator_addr.is_some() && self.executor_addr.is_none() {
            return ExpectedError::IncompleteExecutorSockets {
                given: "orchestrator",
                missing: "executor",
            };
        }
        ExpectedError::NoExecutorSockets
    }
}
