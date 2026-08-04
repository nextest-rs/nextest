// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Driving a run from a spec file instead of from Buck2.
//!
//! A spec file is what Buck2 would have sent over gRPC, captured as JSON. Being
//! able to run from one is useful for working on `buck2-nextest` itself: it
//! reproduces a run without a live Buck2 daemon, and it is how the conversion
//! layer is tested. `buck2-nextest/example/bxl/nextest.bxl` produces one.
//!
//! It is not how Buck2 users run tests, and it cannot do everything the gRPC
//! path can: a spec may contain argument and environment handles, and resolving
//! one means asking Buck2. So this whole module sits behind the non-default
//! `spec-file` feature and is absent from release builds.
//!
//! Everything past building a [`RunContext`] is shared with the gRPC path.

use crate::{
    cli::FilterOpts,
    convert::to_binary_list,
    errors::{ExpectedError, Result},
    run::RunContext,
    spec::read_spec,
};
use camino::Utf8PathBuf;
use clap::{Args, Subcommand, ValueEnum};
use nextest_runner::{list::OutputFormat, platform::BuildPlatforms, write_str::WriteStr};

/// The spec-file subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// List the tests described by a spec.
    List {
        #[command(flatten)]
        spec: SpecOpts,

        #[command(flatten)]
        filter: FilterOpts,

        /// Output format.
        #[arg(long, short = 'T', value_enum, default_value_t = ListFormat::Human)]
        format: ListFormat,
    },

    /// Run the tests described by a spec.
    Run {
        #[command(flatten)]
        spec: SpecOpts,

        #[command(flatten)]
        filter: FilterOpts,
    },
}

impl Command {
    /// Runs the subcommand, returning a process exit code.
    pub fn exec(self, writer: &mut dyn WriteStr, cli_args: Vec<String>) -> Result<i32> {
        match self {
            Command::List {
                spec,
                filter,
                format,
            } => {
                spec.build_context(&filter)?
                    .list(format.to_output_format(), writer)?;
                Ok(0)
            }
            Command::Run { spec, filter } => spec.build_context(&filter)?.run(cli_args),
        }
    }
}

/// Where to read a spec from, and how to interpret its paths.
#[derive(Debug, Args)]
pub struct SpecOpts {
    /// Path to the Buck2 external runner spec, or `-` for standard input.
    #[arg(long, value_name = "PATH")]
    spec: Utf8PathBuf,

    /// The Buck2 project root that relative paths in the spec resolve against.
    #[arg(long, value_name = "PATH", default_value = ".")]
    project_root: Utf8PathBuf,

    /// The nextest profile to use.
    #[arg(long, short = 'P', env = "NEXTEST_PROFILE", value_name = "NAME")]
    profile: Option<String>,

    /// Path to a nextest configuration file.
    #[arg(long, value_name = "PATH")]
    config_file: Option<Utf8PathBuf>,
}

impl SpecOpts {
    fn build_context(&self, filter: &FilterOpts) -> Result<RunContext> {
        let targets = read_spec(&self.spec)?;

        // Buck2 builds for the host, and there is no Cargo config to consult
        // for a target triple, so detect the host platform directly.
        let build_platforms = BuildPlatforms::new_with_no_target().map_err(|error| {
            ExpectedError::HostPlatformDetectError {
                error: Box::new(error),
            }
        })?;

        Ok(RunContext {
            binaries: to_binary_list(&targets, &self.project_root, build_platforms),
            project_root: self.project_root.clone(),
            profile_name: self.profile.clone(),
            config_file: self.config_file.clone(),
            filtersets: filter.filtersets(),
            filter_patterns: filter.patterns(),
            run_ignored: filter.run_ignored(),
            list_threads: filter.list_threads(),
        })
    }
}

/// How `list` should print what it found.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ListFormat {
    /// Human-readable output.
    Human,
    /// One test per line.
    Oneline,
    /// JSON output.
    Json,
    /// Pretty-printed JSON output.
    JsonPretty,
}

impl ListFormat {
    fn to_output_format(self) -> OutputFormat {
        use nextest_runner::list::SerializableFormat;

        match self {
            Self::Human => OutputFormat::Human { verbose: false },
            Self::Oneline => OutputFormat::Oneline { verbose: false },
            Self::Json => OutputFormat::Serializable(SerializableFormat::Json),
            Self::JsonPretty => OutputFormat::Serializable(SerializableFormat::JsonPretty),
        }
    }
}
