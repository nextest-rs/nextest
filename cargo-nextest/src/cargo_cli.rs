// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cargo CLI support.

use crate::output::OutputContext;
use camino::{Utf8Path, Utf8PathBuf};
use clap::Args;
use std::{borrow::Cow, path::PathBuf};

/// Options passed down to cargo.
#[derive(Debug, Args)]
#[command(
    next_help_heading = "Cargo options",
    group = clap::ArgGroup::new("cargo-opts").multiple(true),
)]
pub(crate) struct CargoOptions {
    /// Test only this package's library unit tests
    #[arg(long, group = "cargo-opts")]
    lib: bool,

    /// Test only the specified binary
    #[arg(long, group = "cargo-opts")]
    bin: Vec<String>,

    /// Test all binaries
    #[arg(long, group = "cargo-opts")]
    bins: bool,

    /// Test only the specified example
    #[arg(long, group = "cargo-opts")]
    example: Vec<String>,

    /// Test all examples
    #[arg(long, group = "cargo-opts")]
    examples: bool,

    /// Test only the specified test target
    #[arg(long, group = "cargo-opts")]
    test: Vec<String>,

    /// Test all targets
    #[arg(long, group = "cargo-opts")]
    tests: bool,

    /// Test only the specified bench target
    #[arg(long, group = "cargo-opts")]
    bench: Vec<String>,

    /// Test all benches
    #[arg(long, group = "cargo-opts")]
    benches: bool,

    /// Test all targets
    #[arg(long, group = "cargo-opts")]
    all_targets: bool,

    //  TODO: doc?
    // no-run is handled by test runner
    /// Package to test
    #[arg(short = 'p', long = "package", group = "cargo-opts")]
    packages: Vec<String>,

    /// Build all packages in the workspace
    #[arg(long, group = "cargo-opts")]
    workspace: bool,

    /// Exclude packages from the test
    #[arg(long, group = "cargo-opts")]
    exclude: Vec<String>,

    /// Alias for workspace (deprecated)
    #[arg(long, group = "cargo-opts")]
    all: bool,

    // jobs is handled by test runner
    /// Build artifacts in release mode, with optimizations
    #[arg(long, short = 'r', group = "cargo-opts")]
    release: bool,

    /// Build artifacts with the specified Cargo profile
    #[arg(long, value_name = "NAME", group = "cargo-opts")]
    cargo_profile: Option<String>,

    /// Number of build jobs to run
    #[arg(long, value_name = "JOBS", group = "cargo-opts")]
    build_jobs: Option<String>,

    /// Space or comma separated list of features to activate
    #[arg(long, short = 'F', group = "cargo-opts")]
    features: Vec<String>,

    /// Activate all available features
    #[arg(long, group = "cargo-opts")]
    all_features: bool,

    /// Do not activate the `default` feature
    #[arg(long, group = "cargo-opts")]
    no_default_features: bool,

    /// Build for the target triple
    #[arg(long, value_name = "TRIPLE", group = "cargo-opts")]
    pub(crate) target: Option<String>,

    /// Directory for all generated artifacts
    #[arg(long, value_name = "DIR", group = "cargo-opts")]
    pub(crate) target_dir: Option<Utf8PathBuf>,

    /// Ignore `rust-version` specification in packages
    #[arg(long, group = "cargo-opts")]
    ignore_rust_version: bool,
    // --message-format is captured by nextest
    /// Output build graph in JSON (unstable)
    #[arg(long, group = "cargo-opts")]
    unit_graph: bool,

    /// Outputs a future incompatibility report at the end of the build
    #[arg(long, group = "cargo-opts")]
    future_incompat_report: bool,

    /// Timing output formats (unstable) (comma separated): html, json
    #[arg(long, require_equals = true, value_name = "FMTS", group = "cargo-opts")]
    timings: Option<Option<String>>,

    // --verbose is not currently supported
    // --color is handled by runner
    /// Require Cargo.lock and cache are up to date
    #[arg(long, group = "cargo-opts")]
    frozen: bool,

    /// Require Cargo.lock is up to date
    #[arg(long, group = "cargo-opts")]
    locked: bool,

    /// Run without accessing the network
    #[arg(long, group = "cargo-opts")]
    offline: bool,

    // NOTE: this does not conflict with reuse build opts since we let target.runner be specified
    // this way
    /// Override a configuration value
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) config: Vec<String>,

    /// Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details
    #[clap(short = 'Z', value_name = "FLAG", group = "cargo-opts")]
    unstable_flags: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CargoCli<'a> {
    cargo_path: Utf8PathBuf,
    manifest_path: Option<&'a Utf8Path>,
    output: OutputContext,
    command: &'a str,
    args: Vec<Cow<'a, str>>,
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
        self.args.push(Cow::Borrowed(arg));
        self
    }

    pub(crate) fn add_args(&mut self, args: impl IntoIterator<Item = &'a str>) -> &mut Self {
        self.args.extend(args.into_iter().map(Cow::Borrowed));
        self
    }

    pub(crate) fn add_options(&mut self, options: &'a CargoOptions) -> &mut Self {
        if options.lib {
            self.add_arg("--lib");
        }
        self.add_args(options.bin.iter().flat_map(|s| ["--bin", s.as_str()]));
        if options.bins {
            self.add_arg("--bins");
        }
        self.add_args(
            options
                .example
                .iter()
                .flat_map(|s| ["--example", s.as_str()]),
        );
        if options.examples {
            self.add_arg("--examples");
        }
        self.add_args(options.test.iter().flat_map(|s| ["--test", s.as_str()]));
        if options.tests {
            self.add_arg("--tests");
        }
        self.add_args(options.bench.iter().flat_map(|s| ["--bench", s.as_str()]));
        if options.benches {
            self.add_arg("--benches");
        }
        if options.all_targets {
            self.add_arg("--all-targets");
        }
        self.add_args(
            options
                .packages
                .iter()
                .flat_map(|s| ["--package", s.as_str()]),
        );
        if options.workspace {
            self.add_arg("--workspace");
        }
        self.add_args(
            options
                .exclude
                .iter()
                .flat_map(|s| ["--exclude", s.as_str()]),
        );
        if options.all {
            self.add_arg("--all");
        }
        if options.release {
            self.add_arg("--release");
        }
        if let Some(profile) = &options.cargo_profile {
            self.add_args(["--profile", profile]);
        }
        if let Some(build_jobs) = &options.build_jobs {
            self.add_args(["--jobs", build_jobs.as_str()]);
        }
        self.add_args(options.features.iter().flat_map(|s| ["--features", s]));
        if options.all_features {
            self.add_arg("--all-features");
        }
        if options.no_default_features {
            self.add_arg("--no-default-features");
        }
        if let Some(target) = &options.target {
            self.add_args(["--target", target]);
        }
        if let Some(target_dir) = &options.target_dir {
            self.add_args(["--target-dir", target_dir.as_str()]);
        }
        if options.ignore_rust_version {
            self.add_arg("--ignore-rust-version");
        }
        if options.unit_graph {
            self.add_arg("--unit-graph");
        }
        if let Some(timings) = &options.timings {
            match timings {
                Some(timings) => {
                    // The argument must be passed in as "--timings=html,json", not "--timings
                    // html,json".
                    let timings = format!("--timings={}", timings.as_str());
                    self.add_owned_arg(timings);
                }
                None => {
                    self.add_arg("--timings");
                }
            }
        }
        if options.future_incompat_report {
            self.add_arg("--future-incompat-report");
        }
        if options.frozen {
            self.add_arg("--frozen");
        }
        if options.locked {
            self.add_arg("--locked");
        }
        if options.offline {
            self.add_arg("--offline");
        }
        self.add_args(options.config.iter().flat_map(|s| ["--config", s.as_str()]));
        self.add_args(
            options
                .unstable_flags
                .iter()
                .flat_map(|s| ["-Z", s.as_str()]),
        );

        // TODO: other options

        self
    }

    fn add_owned_arg(&mut self, arg: String) {
        self.args.push(Cow::Owned(arg));
    }

    pub(crate) fn all_args(&self) -> Vec<&str> {
        let mut all_args = vec![self.cargo_path.as_str(), self.command];
        all_args.extend(self.args.iter().map(|s| s.as_ref()));
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
            self.cargo_path.as_str(),
            initial_args
                .into_iter()
                .chain(self.args.iter().map(|s| s.as_ref())),
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
