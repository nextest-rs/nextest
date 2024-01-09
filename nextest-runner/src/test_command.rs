// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_config::EnvironmentMap,
    double_spawn::{DoubleSpawnContext, DoubleSpawnInfo},
    helpers::dylib_path_envvar,
    list::{RustBuildMeta, TestListState},
    test_output::CaptureStrategy,
};
use camino::{Utf8Path, Utf8PathBuf};
use guppy::graph::PackageMetadata;
use once_cell::sync::Lazy;
use std::{
    collections::{BTreeSet, HashMap},
    ffi::{OsStr, OsString},
};

mod imp;
pub use imp::{Child, Output};

#[derive(Clone, Debug)]
pub(crate) struct LocalExecuteContext<'a> {
    // Note: Must use TestListState here to get remapped paths.
    pub(crate) rust_build_meta: &'a RustBuildMeta<TestListState>,
    pub(crate) double_spawn: &'a DoubleSpawnInfo,
    pub(crate) dylib_path: &'a OsStr,
    pub(crate) env: &'a EnvironmentMap,
}

/// Represents a to-be-run test command for a test binary with a certain set of arguments.
pub(crate) struct TestCommand {
    /// The command to be run.
    command: std::process::Command,
    /// Double-spawn context.
    double_spawn: Option<DoubleSpawnContext>,
}

impl TestCommand {
    /// Creates a new test command.
    pub(crate) fn new(
        lctx: &LocalExecuteContext<'_>,
        program: String,
        args: &[&str],
        cwd: &Utf8Path,
        package: &PackageMetadata<'_>,
        non_test_binaries: &BTreeSet<(String, Utf8PathBuf)>,
    ) -> Self {
        let mut cmd = create_command(program, args, lctx.double_spawn);

        // NB: we will always override user-provided environment variables with the
        // `CARGO_*` and `NEXTEST_*` variables set directly on `cmd` below.
        lctx.env.apply_env(&mut cmd);

        cmd.current_dir(cwd)
            // This environment variable is set to indicate that tests are being run under nextest.
            .env("NEXTEST", "1")
            // This environment variable is set to indicate that each test is being run in its own process.
            .env("NEXTEST_EXECUTION_MODE", "process-per-test")
            .env(
                "CARGO_MANIFEST_DIR",
                // CARGO_MANIFEST_DIR is set to the *new* cwd after path mapping.
                cwd,
            );

        apply_package_env(&mut cmd, package);

        apply_ld_dyld_env(&mut cmd, lctx.dylib_path);

        if let Some(out_dir) = lctx
            .rust_build_meta
            .build_script_out_dirs
            .get(package.id().repr())
        {
            // Convert the output directory to an absolute path.
            let out_dir = lctx.rust_build_meta.target_directory.join(out_dir);
            cmd.env("OUT_DIR", out_dir);
        }

        // Expose paths to non-test binaries at runtime so that relocated paths work.
        // These paths aren't exposed by Cargo at runtime, so use a NEXTEST_BIN_EXE prefix.
        for (name, path) in non_test_binaries {
            cmd.env(format!("NEXTEST_BIN_EXE_{name}"), path);
        }

        let double_spawn = lctx.double_spawn.spawn_context();

        Self {
            command: cmd,
            double_spawn,
        }
    }

    #[inline]
    pub(crate) fn command_mut(&mut self) -> &mut std::process::Command {
        &mut self.command
    }

    pub(crate) fn spawn(self, capture_strategy: CaptureStrategy) -> std::io::Result<imp::Child> {
        let res = imp::spawn(self.command, capture_strategy);
        if let Some(ctx) = self.double_spawn {
            ctx.finish();
        }
        res
    }

    pub(crate) async fn wait_with_output(self) -> std::io::Result<std::process::Output> {
        let mut cmd = self.command;
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let res = tokio::process::Command::from(cmd).spawn();

        if let Some(ctx) = self.double_spawn {
            ctx.finish();
        }

        res?.wait_with_output().await
    }
}

pub(crate) fn create_command<I, S>(
    program: String,
    args: I,
    double_spawn: &DoubleSpawnInfo,
) -> std::process::Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<str> + AsRef<OsStr>,
{
    let cmd = if let Some(current_exe) = double_spawn.current_exe() {
        let mut cmd = std::process::Command::new(current_exe);
        cmd.args([DoubleSpawnInfo::SUBCOMMAND_NAME, "--", program.as_str()]);
        cmd.arg(&shell_words::join(args));
        cmd
    } else {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        cmd
    };

    cmd
}

fn apply_package_env(cmd: &mut std::process::Command, package: &PackageMetadata<'_>) {
    cmd.env(
        "__NEXTEST_ORIGINAL_CARGO_MANIFEST_DIR",
        // This is a test-only environment variable set to the *old* cwd. Not part of the
        // public API.
        package.manifest_path().parent().unwrap(),
    )
    // These environment variables are set at runtime by cargo test:
    // https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
    .env("CARGO_PKG_VERSION", format!("{}", package.version()))
    .env(
        "CARGO_PKG_VERSION_MAJOR",
        format!("{}", package.version().major),
    )
    .env(
        "CARGO_PKG_VERSION_MINOR",
        format!("{}", package.version().minor),
    )
    .env(
        "CARGO_PKG_VERSION_PATCH",
        format!("{}", package.version().patch),
    )
    .env(
        "CARGO_PKG_VERSION_PRE",
        format!("{}", package.version().pre),
    )
    .env("CARGO_PKG_AUTHORS", package.authors().join(":"))
    .env("CARGO_PKG_NAME", package.name())
    .env(
        "CARGO_PKG_DESCRIPTION",
        package.description().unwrap_or_default(),
    )
    .env("CARGO_PKG_HOMEPAGE", package.homepage().unwrap_or_default())
    .env("CARGO_PKG_LICENSE", package.license().unwrap_or_default())
    .env(
        "CARGO_PKG_LICENSE_FILE",
        package.license_file().unwrap_or_else(|| "".as_ref()),
    )
    .env(
        "CARGO_PKG_REPOSITORY",
        package.repository().unwrap_or_default(),
    )
    .env(
        "CARGO_PKG_RUST_VERSION",
        package
            .minimum_rust_version()
            .map_or(String::new(), |v| v.to_string()),
    );
}

/// This is a workaround for a macOS SIP issue:
/// https://github.com/nextest-rs/nextest/pull/84
///
/// Basically, if SIP is enabled, macOS removes any environment variables that start with
/// "LD_" or "DYLD_" when spawning system-protected processes. This unfortunately includes
/// processes like bash -- this means that if nextest invokes a shell script, paths might
/// end up getting sanitized.
///
/// This is particularly relevant for target runners, which are often shell scripts.
///
/// To work around this, re-export any variables that begin with LD_ or DYLD_ as "NEXTEST_LD_"
/// or "NEXTEST_DYLD_". Do this on all platforms for uniformity.
///
/// Nextest never changes these environment variables within its own process, so caching them is
/// valid.
pub(crate) fn apply_ld_dyld_env(cmd: &mut std::process::Command, dylib_path: &OsStr) {
    fn is_sip_sanitized(var: &str) -> bool {
        // Look for variables starting with LD_ or DYLD_.
        // https://briandfoy.github.io/macos-s-system-integrity-protection-sanitizes-your-environment/
        var.starts_with("LD_") || var.starts_with("DYLD_")
    }

    static LD_DYLD_ENV_VARS: Lazy<HashMap<String, OsString>> = Lazy::new(|| {
        std::env::vars_os()
            .filter_map(|(k, v)| match k.into_string() {
                Ok(k) => is_sip_sanitized(&k).then_some((k, v)),
                Err(_) => None,
            })
            .collect()
    });

    cmd.env(dylib_path_envvar(), dylib_path);

    // NB: we will always override user-provided environment variables with the
    // `CARGO_*` and `NEXTEST_*` variables set directly on `cmd` below.
    for (k, v) in &*LD_DYLD_ENV_VARS {
        if k != dylib_path_envvar() {
            cmd.env("NEXTEST_".to_owned() + k, v);
        }
    }
    // Also add the dylib path envvar under the NEXTEST_ prefix.
    if is_sip_sanitized(dylib_path_envvar()) {
        cmd.env("NEXTEST_".to_owned() + dylib_path_envvar(), dylib_path);
    }
}
