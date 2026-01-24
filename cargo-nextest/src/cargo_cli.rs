// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cargo CLI support.

use crate::{dispatch::CargoMessageFormatOpt, output::OutputContext};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{ArgAction, Args};
use std::{borrow::Cow, path::PathBuf};

/// Build scope options: package selection, target selection, and feature
/// selection.
///
/// These options determine which packages, targets, and features are built. In
/// a rerun chain, these are inherited from the original run unless explicitly
/// overridden.
#[derive(Clone, Debug, Default, Args)]
pub(crate) struct CargoBuildScopeOptions {
    // ---
    // Package selection
    // ---
    /// Package to test
    #[arg(
        short = 'p',
        long = "package",
        group = "cargo-opts",
        help_heading = "Package selection"
    )]
    packages: Vec<String>,

    /// Test all packages in the workspace
    #[arg(long, group = "cargo-opts", help_heading = "Package selection")]
    workspace: bool,

    /// Exclude packages from the test
    #[arg(long, group = "cargo-opts", help_heading = "Package selection")]
    exclude: Vec<String>,

    /// Alias for --workspace (deprecated)
    #[arg(long, group = "cargo-opts", help_heading = "Package selection")]
    all: bool,

    // ---
    // Target selection
    // ---
    /// Test only this package's library unit tests
    #[arg(long, group = "cargo-opts", help_heading = "Target selection")]
    lib: bool,

    /// Test only the specified binary
    #[arg(long, group = "cargo-opts", help_heading = "Target selection")]
    bin: Vec<String>,

    /// Test all binaries
    #[arg(long, group = "cargo-opts", help_heading = "Target selection")]
    bins: bool,

    /// Test only the specified example
    #[arg(long, group = "cargo-opts", help_heading = "Target selection")]
    example: Vec<String>,

    /// Test all examples
    #[arg(long, group = "cargo-opts", help_heading = "Target selection")]
    examples: bool,

    /// Test only the specified test target
    #[arg(long, group = "cargo-opts", help_heading = "Target selection")]
    test: Vec<String>,

    /// Test all targets
    #[arg(long, group = "cargo-opts", help_heading = "Target selection")]
    tests: bool,

    /// Test only the specified bench target
    #[arg(long, group = "cargo-opts", help_heading = "Target selection")]
    bench: Vec<String>,

    /// Test all benches
    #[arg(long, group = "cargo-opts", help_heading = "Target selection")]
    benches: bool,

    /// Test all targets
    #[arg(long, group = "cargo-opts", help_heading = "Target selection")]
    all_targets: bool,

    // ---
    // Feature selection
    // ---
    /// Space or comma separated list of features to activate
    #[arg(
        long,
        short = 'F',
        group = "cargo-opts",
        help_heading = "Feature selection"
    )]
    features: Vec<String>,

    /// Activate all available features
    #[arg(long, group = "cargo-opts", help_heading = "Feature selection")]
    all_features: bool,

    /// Do not activate the `default` feature
    #[arg(long, group = "cargo-opts", help_heading = "Feature selection")]
    no_default_features: bool,
}

impl CargoBuildScopeOptions {
    /// Returns true if any build scope arguments were provided.
    pub(crate) fn has_any(&self) -> bool {
        let Self {
            // Package selection.
            packages,
            workspace,
            exclude,
            all,
            // Target selection.
            lib,
            bin,
            bins,
            example,
            examples,
            test,
            tests,
            bench,
            benches,
            all_targets,
            // Feature selection.
            features,
            all_features,
            no_default_features,
        } = self;

        !packages.is_empty()
            || *workspace
            || !exclude.is_empty()
            || *all
            || *lib
            || !bin.is_empty()
            || *bins
            || !example.is_empty()
            || *examples
            || !test.is_empty()
            || *tests
            || !bench.is_empty()
            || *benches
            || *all_targets
            || !features.is_empty()
            || *all_features
            || *no_default_features
    }

    /// Converts build scope args to a list of CLI arguments for storage.
    pub(crate) fn to_cli_args(&self) -> Vec<&str> {
        let Self {
            // Package selection.
            packages,
            workspace,
            exclude,
            all,
            // Target selection.
            lib,
            bin,
            bins,
            example,
            examples,
            test,
            tests,
            bench,
            benches,
            all_targets,
            // Feature selection.
            features,
            all_features,
            no_default_features,
        } = self;

        let mut args = Vec::new();

        // Package selection.
        for pkg in packages {
            args.extend(["--package", pkg.as_str()]);
        }
        if *workspace {
            args.push("--workspace");
        }
        for exc in exclude {
            args.extend(["--exclude", exc.as_str()]);
        }
        if *all {
            args.push("--all");
        }

        // Target selection.
        if *lib {
            args.push("--lib");
        }
        for b in bin {
            args.extend(["--bin", b.as_str()]);
        }
        if *bins {
            args.push("--bins");
        }
        for ex in example {
            args.extend(["--example", ex.as_str()]);
        }
        if *examples {
            args.push("--examples");
        }
        for t in test {
            args.extend(["--test", t.as_str()]);
        }
        if *tests {
            args.push("--tests");
        }
        for b in bench {
            args.extend(["--bench", b.as_str()]);
        }
        if *benches {
            args.push("--benches");
        }
        if *all_targets {
            args.push("--all-targets");
        }

        // Feature selection.
        for f in features {
            args.extend(["--features", f.as_str()]);
        }
        if *all_features {
            args.push("--all-features");
        }
        if *no_default_features {
            args.push("--no-default-features");
        }

        args
    }
}

/// Options passed down to cargo.
#[derive(Debug, Default, Args)]
#[command(
    group = clap::ArgGroup::new("cargo-opts").multiple(true),
)]
pub(crate) struct CargoOptions {
    /// Build scope options (package, target, and feature selection).
    #[command(flatten)]
    pub(crate) build_scope: CargoBuildScopeOptions,

    // jobs is handled by test runner
    /// Number of build jobs to run
    #[arg(
        long,
        value_name = "N",
        group = "cargo-opts",
        help_heading = "Compilation options",
        allow_negative_numbers = true
    )]
    build_jobs: Option<String>,

    /// Build artifacts in release mode, with optimizations
    #[arg(
        long,
        short = 'r',
        group = "cargo-opts",
        help_heading = "Compilation options"
    )]
    release: bool,

    /// Build artifacts with the specified Cargo profile
    #[arg(
        long,
        // This is shortened from PROFILE-NAME to NAME to reduce option column width.
        value_name = "NAME",
        group = "cargo-opts",
        help_heading = "Compilation options"
    )]
    cargo_profile: Option<String>,

    /// Build for the target triple
    #[arg(
        long,
        value_name = "TRIPLE",
        group = "cargo-opts",
        help_heading = "Compilation options"
    )]
    pub(crate) target: Option<String>,

    /// Directory for all generated artifacts
    #[arg(
        long,
        value_name = "DIR",
        group = "cargo-opts",
        help_heading = "Compilation options"
    )]
    pub(crate) target_dir: Option<Utf8PathBuf>,

    /// Output build graph in JSON (unstable)
    #[arg(long, group = "cargo-opts", help_heading = "Compilation options")]
    unit_graph: bool,

    /// Timing output formats (unstable) (comma separated): html, json
    #[arg(
        long,
        require_equals = true,
        value_name = "FMTS",
        group = "cargo-opts",
        help_heading = "Compilation options"
    )]
    timings: Option<Option<String>>,

    // --color is handled by runner
    /// Require Cargo.lock and cache are up to date
    #[arg(long, group = "cargo-opts", help_heading = "Manifest options")]
    frozen: bool,

    /// Require Cargo.lock is up to date
    #[arg(long, group = "cargo-opts", help_heading = "Manifest options")]
    locked: bool,

    /// Run without accessing the network
    #[arg(long, group = "cargo-opts", help_heading = "Manifest options")]
    offline: bool,

    /// Cargo message format.
    ///
    /// Controls how Cargo reports build messages and whether to forward JSON to
    /// stdout. Multiple JSON options can be comma-separated or repeated.
    #[arg(
        long,
        value_enum,
        value_name = "FMT",
        value_delimiter = ',',
        group = "cargo-opts",
        help_heading = "Other Cargo options"
    )]
    pub(crate) cargo_message_format: Vec<CargoMessageFormatOpt>,

    //  TODO: doc?
    // no-run is handled by test runner
    /// Do not print cargo log messages (specify twice for no Cargo output at all)
    #[arg(long, action = ArgAction::Count, group = "cargo-opts", help_heading = "Other Cargo options")]
    cargo_quiet: u8,

    /// Use cargo verbose output (specify twice for very verbose/build.rs output)
    #[arg(long, action = ArgAction::Count, group = "cargo-opts", help_heading = "Other Cargo options")]
    cargo_verbose: u8,

    /// Ignore `rust-version` specification in packages
    #[arg(long, group = "cargo-opts", help_heading = "Other Cargo options")]
    ignore_rust_version: bool,
    // --message-format is captured by nextest
    /// Outputs a future incompatibility report at the end of the build
    #[arg(long, group = "cargo-opts", help_heading = "Other Cargo options")]
    future_incompat_report: bool,

    // NOTE: this does not conflict with reuse build opts (not part of the cargo-opts group) since
    // we let target.runner be specified this way
    /// Override a Cargo configuration value
    #[arg(long, value_name = "KEY=VALUE", help_heading = "Other Cargo options")]
    pub(crate) config: Vec<String>,

    /// Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details
    #[clap(
        short = 'Z',
        value_name = "FLAG",
        group = "cargo-opts",
        help_heading = "Other Cargo options"
    )]
    unstable_flags: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CargoCli<'a> {
    cargo_path: Utf8PathBuf,
    manifest_path: Option<&'a Utf8Path>,
    output: OutputContext,
    command: &'a str,
    args: Vec<Cow<'a, str>>,
    stderr_null: bool,
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
            stderr_null: false,
        }
    }

    pub(crate) fn add_arg(&mut self, arg: &'a str) -> &mut Self {
        self.args.push(Cow::Borrowed(arg));
        self
    }

    pub(crate) fn add_args(&mut self, args: impl IntoIterator<Item = &'a str>) -> &mut Self {
        self.args.extend(args.into_iter().map(Cow::Borrowed));
        self
    }

    /// Adds build scope options (package, target, and feature selection) to the
    /// command.
    pub(crate) fn add_build_scope_options(
        &mut self,
        scope: &'a CargoBuildScopeOptions,
    ) -> &mut Self {
        for arg in scope.to_cli_args() {
            self.add_arg(arg);
        }
        self
    }

    pub(crate) fn add_options(&mut self, options: &'a CargoOptions) -> &mut Self {
        self.add_build_scope_options(&options.build_scope);
        self.add_non_build_scope_options(options);
        self
    }

    /// Adds non-build-scope options from `CargoOptions`.
    ///
    /// This includes compilation options, manifest options, and other cargo
    /// options. Used when inheriting build scope from a rerun but using the
    /// current run's other options.
    pub(crate) fn add_non_build_scope_options(&mut self, options: &'a CargoOptions) -> &mut Self {
        // ---
        // Compilation options
        // ---
        if let Some(build_jobs) = &options.build_jobs {
            self.add_args(["--jobs", build_jobs.as_str()]);
        }
        if options.release {
            self.add_arg("--release");
        }
        if let Some(profile) = &options.cargo_profile {
            self.add_args(["--profile", profile]);
        }
        if let Some(target) = &options.target {
            self.add_args(["--target", target]);
        }
        if let Some(target_dir) = &options.target_dir {
            self.add_args(["--target-dir", target_dir.as_str()]);
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

        self.add_generic_cargo_options(options);

        // ---
        // Other Cargo options
        // ---

        if options.cargo_verbose > 0 {
            self.add_args(std::iter::repeat_n(
                "--verbose",
                options.cargo_verbose.into(),
            ));
        }
        if options.ignore_rust_version {
            self.add_arg("--ignore-rust-version");
        }
        if options.future_incompat_report {
            self.add_arg("--future-incompat-report");
        }
        self.add_args(
            options
                .unstable_flags
                .iter()
                .flat_map(|s| ["-Z", s.as_str()]),
        );

        self
    }

    /// Add Cargo options that are common to all commands.
    pub(crate) fn add_generic_cargo_options(&mut self, options: &'a CargoOptions) -> &mut Self {
        // ---
        // Manifest options
        // ---
        if options.frozen {
            self.add_arg("--frozen");
        }
        if options.locked {
            self.add_arg("--locked");
        }
        if options.offline {
            self.add_arg("--offline");
        }

        // Other cargo options. We don't apply --verbose here, since we generally intend --verbose
        // to only be for the main build.
        if options.cargo_quiet > 0 {
            self.add_arg("--quiet");
        }
        if options.cargo_quiet > 1 {
            self.stderr_null = true;
        }
        self.add_args(options.config.iter().flat_map(|s| ["--config", s.as_str()]));

        self
    }

    pub(crate) fn add_owned_arg(&mut self, arg: String) {
        self.args.push(Cow::Owned(arg));
    }

    pub(crate) fn all_args(&self) -> Vec<&str> {
        let mut all_args = vec![self.cargo_path.as_str(), self.command];
        all_args.extend(self.args.iter().map(|s| &**s));
        all_args
    }

    pub(crate) fn to_expression(&self) -> duct::Expression {
        let mut initial_args = vec![self.output.color.to_arg(), self.command];
        if let Some(path) = self.manifest_path {
            initial_args.extend(["--manifest-path", path.as_str()]);
        }
        let ret = duct::cmd(
            // Ensure that cargo gets picked up from PATH if necessary, by calling as_str
            // rather than as_std_path.
            self.cargo_path.as_str(),
            initial_args
                .into_iter()
                .chain(self.args.iter().map(|s| s.as_ref())),
        );

        if self.stderr_null {
            ret.stderr_null()
        } else {
            ret
        }
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
