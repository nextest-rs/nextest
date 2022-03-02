// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cargo CLI support.

use crate::output::OutputContext;
use camino::{Utf8Path, Utf8PathBuf};
use clap::{AppSettings, Args};
use std::{convert::TryInto, path::PathBuf};

/// Options passed down to cargo.
#[derive(Debug, Args)]
#[clap(next_help_heading = "CARGO OPTIONS", setting = AppSettings::DeriveDisplayOrder)]
pub(crate) struct CargoOptions {
    /// Test only this package's library unit tests
    #[clap(long)]
    lib: bool,

    /// Test only the specified binary
    #[clap(long)]
    bin: Vec<String>,

    /// Test all binaries
    #[clap(long)]
    bins: bool,

    /// Test only the specified test target
    #[clap(long)]
    test: Vec<String>,

    /// Test all targets
    #[clap(long)]
    tests: bool,

    /// Test only the specified bench target
    #[clap(long)]
    bench: Vec<String>,

    /// Test all benches
    #[clap(long)]
    benches: bool,

    /// Test all targets
    #[clap(long)]
    all_targets: bool,

    //  TODO: doc?
    // no-run is handled by test runner
    /// Package to test
    #[clap(short = 'p', long = "package")]
    packages: Vec<String>,

    /// Build all packages in the workspace
    #[clap(long)]
    workspace: bool,

    /// Exclude packages from the test
    #[clap(long)]
    exclude: Vec<String>,

    /// Alias for workspace (deprecated)
    #[clap(long)]
    all: bool,

    // jobs is handled by test runner
    /// Build artifacts in release mode, with optimizations
    #[clap(long)]
    release: bool,

    /// Build artifacts with the specified Cargo profile
    #[clap(long, value_name = "NAME")]
    cargo_profile: Option<String>,

    /// Number of build jobs to run
    #[clap(long, value_name = "JOBS")]
    build_jobs: Option<String>,

    /// Space or comma separated list of features to activate
    #[clap(long)]
    features: Vec<String>,

    /// Activate all available features
    #[clap(long)]
    all_features: bool,

    /// Do not activate the `default` feature
    #[clap(long)]
    no_default_features: bool,

    /// Build for the target triple
    #[clap(long, value_name = "TRIPLE")]
    pub(crate) target: Option<String>,

    /// Directory for all generated artifacts
    #[clap(long, value_name = "DIR")]
    target_dir: Option<String>,

    /// Ignore `rust-version` specification in packages
    #[clap(long)]
    ignore_rust_version: bool,
    // --message-format is captured by nextest
    /// Output build graph in JSON (unstable)
    #[clap(long)]
    unit_graph: bool,

    /// Outputs a future incompatibility report at the end of the build (unstable)
    #[clap(long)]
    future_incompat_report: bool,

    // --verbose is not currently supported
    // --color is handled by runner
    /// Require Cargo.lock and cache are up to date
    #[clap(long)]
    frozen: bool,

    /// Require Cargo.lock is up to date
    #[clap(long)]
    locked: bool,

    /// Run without accessing the network
    #[clap(long)]
    offline: bool,

    /// Override a configuration value (unstable)
    #[clap(long, value_name = "KEY=VALUE")]
    config: Vec<String>,

    /// Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details
    #[clap(short = 'Z', value_name = "FLAG")]
    unstable_flags: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CargoCli<'a> {
    cargo_path: Utf8PathBuf,
    manifest_path: Option<&'a Utf8Path>,
    output: OutputContext,
    command: &'a str,
    args: Vec<&'a str>,
}

impl<'a> CargoCli<'a> {
    pub(crate) fn new(
        command: &'a str,
        manifest_path: Option<&'a Utf8Path>,
        output: OutputContext,
    ) -> Self {
        let cargo_path = cargo_path();
        Self {
            cargo_path,
            manifest_path,
            output,
            command,
            args: vec![],
        }
    }

    #[allow(dead_code)]
    pub(crate) fn add_arg(&mut self, arg: &'a str) -> &mut Self {
        self.args.push(arg);
        self
    }

    pub(crate) fn add_args(&mut self, args: impl IntoIterator<Item = &'a str>) -> &mut Self {
        self.args.extend(args);
        self
    }

    pub(crate) fn add_options(&mut self, options: &'a CargoOptions) -> &mut Self {
        if options.lib {
            self.args.push("--lib");
        }
        self.args
            .extend(options.bin.iter().flat_map(|s| ["--bin", s.as_str()]));
        if options.bins {
            self.args.push("--bins");
        }
        self.args
            .extend(options.test.iter().flat_map(|s| ["--test", s.as_str()]));
        if options.tests {
            self.args.push("--tests");
        }
        self.args
            .extend(options.bench.iter().flat_map(|s| ["--bench", s.as_str()]));
        if options.benches {
            self.args.push("--benches");
        }
        if options.all_targets {
            self.args.push("--all-targets");
        }
        self.args.extend(
            options
                .packages
                .iter()
                .flat_map(|s| ["--package", s.as_str()]),
        );
        if options.workspace {
            self.args.push("--workspace");
        }
        self.args.extend(
            options
                .exclude
                .iter()
                .flat_map(|s| ["--exclude", s.as_str()]),
        );
        if options.all {
            self.args.push("--all");
        }
        if options.release {
            self.args.push("--release");
        }
        if let Some(profile) = &options.cargo_profile {
            self.args.extend(["--profile", profile]);
        }
        if let Some(build_jobs) = &options.build_jobs {
            self.args.extend(["--jobs", build_jobs.as_str()]);
        }
        self.args
            .extend(options.features.iter().flat_map(|s| ["--features", s]));
        if options.all_features {
            self.args.push("--all-features");
        }
        if options.no_default_features {
            self.args.push("--no-default-features");
        }
        if let Some(target) = &options.target {
            self.args.extend(["--target", target]);
        }
        if let Some(target_dir) = &options.target_dir {
            self.args.extend(["--target-dir", target_dir]);
        }
        if options.ignore_rust_version {
            self.args.push("--ignore-rust-version");
        }
        if options.unit_graph {
            self.args.push("--unit-graph");
        }
        if options.future_incompat_report {
            self.args.push("--future-incompat-report");
        }
        if options.frozen {
            self.args.push("--frozen");
        }
        if options.locked {
            self.args.push("--locked");
        }
        if options.offline {
            self.args.push("--offline");
        }
        self.args
            .extend(options.config.iter().flat_map(|s| ["--config", s.as_str()]));
        self.args.extend(
            options
                .unstable_flags
                .iter()
                .flat_map(|s| ["-Z", s.as_str()]),
        );

        // TODO: other options

        self
    }

    pub(crate) fn all_args(&self) -> Vec<&str> {
        let mut all_args = vec![self.cargo_path.as_str(), self.command];
        all_args.extend_from_slice(&self.args);
        all_args
    }

    pub(crate) fn to_expression(&self) -> duct::Expression {
        let mut initial_args = vec![self.output.color.to_arg(), self.command];
        if let Some(path) = self.manifest_path {
            initial_args.extend(["--manifest-path", path.as_str()]);
        }
        duct::cmd(
            // Ensure that cargo gets picked up from PATH if necessary, by calling as_str
            // rather than as_std_path.
            self.cargo_path.as_std_path(),
            initial_args.into_iter().chain(self.args.iter().copied()),
        )
    }
}

fn cargo_path() -> Utf8PathBuf {
    match std::env::var_os("CARGO") {
        Some(cargo_path) => PathBuf::from(cargo_path)
            .try_into()
            .expect("CARGO env var is not valid UTF-8"),
        None => Utf8PathBuf::from("cargo"),
    }
}
