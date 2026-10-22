// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cargo CLI support.

use crate::{dispatch::CargoMessageFormatOpt, output::OutputContext};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{ArgAction, Args};
use nextest_runner::build_scope::{
    BuildScope, CargoFlag, FeatureSelection, NamedTargets, PackageSelection, PackageSpec,
    TargetSelection,
};
use std::{borrow::Cow, fmt, path::PathBuf};

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
    packages: Vec<PackageSpec>,

    /// Test all packages in the workspace
    #[arg(long, group = "cargo-opts", help_heading = "Package selection")]
    workspace: bool,

    /// Exclude packages from the test
    #[arg(long, group = "cargo-opts", help_heading = "Package selection")]
    exclude: Vec<PackageSpec>,

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

/// Options passed down to cargo.
#[derive(Debug, Default, Args)]
#[command(
    group = clap::ArgGroup::new("cargo-opts").multiple(true),
)]
pub(crate) struct CargoOptions {
    /// Build scope options (package, target, and feature selection).
    #[command(flatten)]
    build_scope: CargoBuildScopeOptions,

    #[command(flatten)]
    settings: CargoBuildSettings,
}

impl CargoOptions {
    pub(crate) fn into_parts(self) -> (BuildScope, CargoBuildSettings) {
        let Self {
            build_scope,
            settings,
        } = self;
        let CargoBuildScopeOptions {
            packages,
            workspace,
            exclude,
            all,
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
            features,
            all_features,
            no_default_features,
        } = build_scope;

        let build_scope = BuildScope::new(
            PackageSelection {
                specs: packages,
                // --all is a deprecated alias for --workspace.
                workspace: workspace || all,
                exclude,
            },
            TargetSelection {
                lib,
                bins: NamedTargets {
                    names: bin,
                    all: bins,
                },
                examples: NamedTargets {
                    names: example,
                    all: examples,
                },
                tests: NamedTargets {
                    names: test,
                    all: tests,
                },
                benches: NamedTargets {
                    names: bench,
                    all: benches,
                },
                all_targets,
            },
            FeatureSelection {
                features,
                all_features,
                no_default_features,
            },
        );
        (build_scope, settings)
    }
}

/// All Cargo options other than build scope options.
#[derive(Debug, Default, Args)]
pub(crate) struct CargoBuildSettings {
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
    output: OutputContext,
    command: &'a str,
    args: Vec<Cow<'static, str>>,
    stderr_null: bool,
}

impl<'a> CargoCli<'a> {
    pub(crate) fn new(
        command: &'a str,
        manifest_path: Option<&Utf8Path>,
        output: OutputContext,
    ) -> Self {
        let cargo_path = cargo_path();
        let mut cli = Self {
            cargo_path,
            output,
            command,
            args: vec![],
            stderr_null: false,
        };
        if let Some(manifest_path) = manifest_path {
            cli.add_flag_value("--manifest-path", manifest_path);
        }
        cli
    }

    pub(crate) fn add_arg(&mut self, arg: &'static str) -> &mut Self {
        self.args.push(Cow::Borrowed(arg));
        self
    }

    pub(crate) fn add_args(&mut self, args: impl IntoIterator<Item = &'static str>) -> &mut Self {
        self.args.extend(args.into_iter().map(Cow::Borrowed));
        self
    }

    pub(crate) fn add_flag_value(
        &mut self,
        flag: &'static str,
        value: impl fmt::Display,
    ) -> &mut Self {
        let value = value.to_string();
        self.add_owned_arg(
            CargoFlag::Valued {
                flag,
                value: &value,
            }
            .to_cargo_arg(),
        )
    }

    /// Adds build scope options (package, target, and feature selection) to the
    /// command.
    pub(crate) fn add_build_scope(&mut self, scope: &BuildScope) -> &mut Self {
        for flag in scope.cargo_flags() {
            match flag {
                CargoFlag::Bare(arg) => {
                    self.add_arg(arg);
                }
                CargoFlag::Valued { .. } => {
                    self.add_owned_arg(flag.to_cargo_arg());
                }
            }
        }
        self
    }

    /// Adds Cargo options (build scope and otherwise) to this Cargo command.
    ///
    /// Here, `build_scope` is the effective scope: either the one provided
    /// during this run, or one a rerun inherits from its original run.
    pub(crate) fn add_options(
        &mut self,
        build_scope: &BuildScope,
        settings: &CargoBuildSettings,
    ) -> &mut Self {
        self.add_build_scope(build_scope);
        self.add_build_settings(settings);
        self
    }

    /// Adds non-build-scope options from `CargoBuildSettings`.
    ///
    /// This includes compilation options, manifest options, and other cargo
    /// options.
    pub(crate) fn add_build_settings(&mut self, options: &CargoBuildSettings) -> &mut Self {
        // ---
        // Compilation options
        // ---
        if let Some(build_jobs) = &options.build_jobs {
            self.add_flag_value("--jobs", build_jobs);
        }
        if options.release {
            self.add_arg("--release");
        }
        if let Some(profile) = &options.cargo_profile {
            self.add_flag_value("--profile", profile);
        }
        if let Some(target) = &options.target {
            self.add_flag_value("--target", target);
        }
        if let Some(target_dir) = &options.target_dir {
            self.add_flag_value("--target-dir", target_dir);
        }
        if options.unit_graph {
            self.add_arg("--unit-graph");
        }
        if let Some(timings) = &options.timings {
            match timings {
                Some(timings) => {
                    self.add_flag_value("--timings", timings);
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
        for flag in &options.unstable_flags {
            self.add_owned_arg(format!("-Z{flag}"));
        }

        self
    }

    /// Add Cargo options that are common to all commands.
    pub(crate) fn add_generic_cargo_options(&mut self, options: &CargoBuildSettings) -> &mut Self {
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
        for config in &options.config {
            self.add_flag_value("--config", config);
        }

        self
    }

    /// Adds an owned argument.
    ///
    /// Do not expose this function outside this module! This is private so that
    /// dynamic values from outside this module can only be added when combined
    /// with a flag via `=` (via `add_flag_value` or `add_build_scope`).
    /// Non-equals Cargo arguments are susceptible to injection attacks (e.g.
    /// Cargo accepts `-p --config=...`).
    fn add_owned_arg(&mut self, arg: String) -> &mut Self {
        self.args.push(Cow::Owned(arg));
        self
    }

    pub(crate) fn all_args(&self) -> Vec<&str> {
        let mut all_args = vec![
            self.cargo_path.as_str(),
            self.output.color.to_arg(),
            self.command,
        ];
        all_args.extend(self.args.iter().map(|s| &**s));
        all_args
    }

    pub(crate) fn to_expression(&self) -> duct::Expression {
        let all_args = self.all_args();
        let (program, args) = all_args
            .split_first()
            .expect("all_args always begins with the cargo path");
        // Ensure that cargo gets picked up from PATH if necessary, by calling as_str
        // rather than as_std_path.
        let ret = duct::cmd(*program, args);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Color;
    use clap::Parser;
    use insta::assert_debug_snapshot;

    #[derive(Debug, Parser)]
    struct CargoOptionsCli {
        #[command(flatten)]
        cargo_options: CargoOptions,
    }

    #[test]
    fn to_expression_uses_all_args() {
        // Ensure that all_args is complete.
        let mut cargo_cli = CargoCli::new(
            "metadata",
            Some(Utf8Path::new("path/to/Cargo.toml")),
            OutputContext {
                verbose: false,
                color: Color::Never,
            },
        );
        cargo_cli
            .add_args(["--format-version=1", "--all-features"])
            .add_flag_value("--jobs", 4);

        let all_args = cargo_cli.all_args();
        assert_eq!(all_args[0], cargo_path().as_str());
        assert_eq!(
            &all_args[1..],
            [
                "--color=never",
                "metadata",
                "--manifest-path=path/to/Cargo.toml",
                "--format-version=1",
                "--all-features",
                "--jobs=4",
            ]
        );
        assert_eq!(
            format!("{:?}", cargo_cli.to_expression()),
            format!("Cmd({all_args:?})"),
        );

        // A command without --manifest-path.
        let cargo_cli = CargoCli::new(
            "locate-project",
            None,
            OutputContext {
                verbose: false,
                color: Color::Always,
            },
        );
        assert_eq!(
            &cargo_cli.all_args()[1..],
            ["--color=always", "locate-project"]
        );
    }

    #[test]
    fn add_options_all_settings_set() {
        let spec = |spec: &str| PackageSpec::new(spec).expect("spec is valid");
        let options = CargoOptions {
            // Use some values starting with `-` to show in the snapshot that
            // each value stays bound to its flag via `=`.
            build_scope: CargoBuildScopeOptions {
                packages: vec![spec("foo"), spec("bar@1.2")],
                workspace: true,
                exclude: vec![spec("baz")],
                all: false,
                lib: true,
                bin: vec!["my-bin".to_owned()],
                bins: true,
                example: vec!["-dash-example".to_owned()],
                examples: true,
                test: vec!["-dash-test".to_owned()],
                tests: true,
                bench: vec!["my-bench".to_owned()],
                benches: true,
                all_targets: true,
                features: vec!["feat-a feat-b".to_owned(), "-dash-feature".to_owned()],
                all_features: true,
                no_default_features: true,
            },
            settings: CargoBuildSettings {
                build_jobs: Some("-1".to_owned()),
                release: true,
                cargo_profile: Some("my-profile".to_owned()),
                target: Some("x86_64-unknown-linux-gnu".to_owned()),
                target_dir: Some(Utf8PathBuf::from("path/to/target dir")),
                unit_graph: true,
                timings: Some(Some("html,json".to_owned())),
                frozen: true,
                locked: true,
                offline: true,
                // The message format is added by compute_binary_list, not here.
                cargo_message_format: Vec::new(),
                cargo_quiet: 1,
                cargo_verbose: 2,
                ignore_rust_version: true,
                future_incompat_report: true,
                config: vec![
                    "build.rustflags=[\"-Dwarnings\"]".to_owned(),
                    "path/to/config.toml".to_owned(),
                ],
                unstable_flags: vec!["unstable-options".to_owned(), "build-std".to_owned()],
            },
        };

        let mut cargo_cli = CargoCli::new(
            "test",
            Some(Utf8Path::new("path/to/Cargo.toml")),
            OutputContext {
                verbose: false,
                color: Color::Never,
            },
        );
        let (build_scope, settings) = options.into_parts();
        cargo_cli.add_options(&build_scope, &settings);

        assert_debug_snapshot!(&cargo_cli.all_args()[1..]);
    }

    #[test]
    fn build_scope_from_argv() {
        let cases: &[(&str, &[&str])] = &[
            ("", &[]),
            (
                "-p foo -p bar@1.2 --exclude baz --workspace",
                &[
                    "--package=foo",
                    "--package=bar@1.2",
                    "--workspace",
                    "--exclude=baz",
                ],
            ),
            ("--all", &["--workspace"]),
            ("--workspace --all", &["--workspace"]),
            (
                "--package=nextest-* --all",
                &["--package=nextest-*", "--workspace"],
            ),
            (
                "-F a,b --no-default-features",
                &["--features=a,b", "--no-default-features"],
            ),
            (
                "--features 'a b' --all-features",
                &["--features=a b", "--all-features"],
            ),
            ("--lib --bins --test t", &["--lib", "--bins", "--test=t"]),
            (
                "--bin b --example e --examples --tests --bench be --benches --all-targets",
                &[
                    "--bin=b",
                    "--example=e",
                    "--examples",
                    "--tests",
                    "--bench=be",
                    "--benches",
                    "--all-targets",
                ],
            ),
        ];

        for (args, expected) in cases {
            let argv = shell_words::split(&format!("cargo-nextest {args}"))
                .expect("test command line is valid shell syntax");
            let cli = CargoOptionsCli::try_parse_from(argv)
                .unwrap_or_else(|error| panic!("`{args}` should parse: {error}"));
            let scope = cli.cargo_options.into_parts().0;
            assert_eq!(scope.to_cargo_args(), *expected, "cargo args for `{args}`");
            assert_eq!(
                scope.is_empty(),
                expected.is_empty(),
                "is_empty for `{args}`"
            );
        }
    }
}
