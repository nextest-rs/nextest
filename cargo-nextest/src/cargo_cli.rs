// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cargo CLI support.

use crate::output::OutputContext;
use camino::{Utf8Path, Utf8PathBuf};
use clap::Args;
use guppy::graph::PackageGraph;
use std::path::PathBuf;

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

impl CargoOptions {
    fn needs_default_members_warning(&self, graph: &PackageGraph) -> bool {
        // If any package-related options are passed in, don't produce a warning.
        if !self.packages.is_empty() || self.workspace || self.all {
            return false;
        }

        // TODO: figure out whether this will cause everything to be built. Factors to consider:
        // * symlinks
        // * exact behavior with nested directories
    }
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
        self.args.extend(
            options
                .example
                .iter()
                .flat_map(|s| ["--example", s.as_str()]),
        );
        if options.examples {
            self.args.push("--examples");
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
            self.args.extend(["--target-dir", target_dir.as_str()]);
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
            self.cargo_path.as_str(),
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
