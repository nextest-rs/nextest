// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_config::EnvironmentMap,
    double_spawn::{DoubleSpawnContext, DoubleSpawnInfo},
    helpers::dylib_path_envvar,
    list::{RustBuildMeta, TestListState},
    runner::Interceptor,
    test_output::CaptureStrategy,
};
use camino::{Utf8Path, Utf8PathBuf};
use guppy::graph::PackageMetadata;
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    ffi::{OsStr, OsString},
    fs::File,
    io::{BufRead, BufReader},
    sync::LazyLock,
};
use tracing::warn;

mod imp;
pub(crate) use imp::{Child, ChildAccumulator, ChildFds};

#[derive(Clone, Debug)]
pub(crate) struct LocalExecuteContext<'a> {
    pub(crate) phase: TestCommandPhase,
    pub(crate) workspace_root: &'a Utf8Path,
    // Note: Must use TestListState here to get remapped paths.
    pub(crate) rust_build_meta: &'a RustBuildMeta<TestListState>,
    pub(crate) double_spawn: &'a DoubleSpawnInfo,
    pub(crate) dylib_path: &'a OsStr,
    pub(crate) profile_name: &'a str,
    pub(crate) env: &'a EnvironmentMap,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum TestCommandPhase {
    List,
    Run,
}

/// Represents a to-be-run test command for a test binary with a certain set of arguments.
pub(crate) struct TestCommand {
    /// The program to run.
    program: String,
    /// The arguments to pass to the program.
    args: Vec<String>,
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
        args: &[Cow<'_, str>],
        cwd: &Utf8Path,
        package: &PackageMetadata<'_>,
        non_test_binaries: &BTreeSet<(String, Utf8PathBuf)>,
        interceptor: &Interceptor,
    ) -> Self {
        let mut cmd = if interceptor.should_show_wrapper_command() {
            create_command_with_interceptor(program.clone(), args, interceptor)
        } else {
            create_command(program.clone(), args, lctx.double_spawn)
        };

        // NB: we will always override user-provided environment variables with the
        // `CARGO_*` and `NEXTEST_*` variables set directly on `cmd` below.
        lctx.env.apply_env(&mut cmd);

        if let Some(out_dir) = lctx
            .rust_build_meta
            .build_script_out_dirs
            .get(package.id().repr())
        {
            // Convert the output directory to an absolute path.
            let out_dir = lctx.rust_build_meta.target_directory.join(out_dir);
            cmd.env("OUT_DIR", &out_dir);

            // Apply the user-provided environment variables from the build script. This is
            // supported by cargo test, but discouraged.
            apply_build_script_env(&mut cmd, &out_dir);
        }

        cmd.current_dir(cwd)
            // This environment variable is set to indicate that tests are being run under nextest.
            .env("NEXTEST", "1")
            // This environment variable is set to indicate that each test is being run in its own process.
            .env("NEXTEST_EXECUTION_MODE", "process-per-test")
            // Set the nextest profile.
            .env("NEXTEST_PROFILE", lctx.profile_name)
            .env(
                "CARGO_MANIFEST_DIR",
                // CARGO_MANIFEST_DIR is set to the *new* cwd after path mapping.
                cwd,
            );
        match lctx.phase {
            TestCommandPhase::List => {
                cmd.env("NEXTEST_TEST_PHASE", "list");
            }
            TestCommandPhase::Run => {
                cmd.env("NEXTEST_TEST_PHASE", "run");
            }
        }

        apply_package_env(&mut cmd, package);

        apply_ld_dyld_env(&mut cmd, lctx.dylib_path);

        // Expose paths to non-test binaries at runtime so that relocated paths
        // work. These paths aren't exposed by Cargo at runtime, so use a
        // NEXTEST_BIN_EXE prefix.
        for (name, path) in non_test_binaries {
            // Some shells and debuggers have been known to drop environment
            // variables with hyphens in their names. Provide an alternative
            // name with underscores instead.
            //
            // See
            // https://unix.stackexchange.com/questions/23659/can-shell-variable-name-include-a-hyphen-or-dash.
            let with_underscores = name.replace('-', "_");
            cmd.env(format!("NEXTEST_BIN_EXE_{name}"), path);
            if &with_underscores != name {
                cmd.env(format!("NEXTEST_BIN_EXE_{with_underscores}"), path);
            }
        }

        let double_spawn = lctx.double_spawn.spawn_context();

        Self {
            program,
            args: args.iter().map(|arg| arg.clone().into_owned()).collect(),
            command: cmd,
            double_spawn,
        }
    }

    #[allow(unused)]
    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    #[allow(unused)]
    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }

    #[inline]
    pub(crate) fn command_mut(&mut self) -> &mut std::process::Command {
        &mut self.command
    }

    pub(crate) fn spawn(
        self,
        capture_strategy: CaptureStrategy,
        stdin_passthrough: bool,
    ) -> std::io::Result<imp::Child> {
        let res = imp::spawn(self.command, capture_strategy, stdin_passthrough);
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
    S: AsRef<str>,
{
    if let Some(current_exe) = double_spawn.current_exe() {
        let mut cmd = std::process::Command::new(current_exe);
        cmd.args([DoubleSpawnInfo::SUBCOMMAND_NAME, "--", program.as_str()]);
        cmd.arg(shell_words::join(args));
        cmd
    } else {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        cmd
    }
}

pub(crate) fn create_command_with_interceptor<I, S>(
    program: String,
    args: I,
    interceptor: &Interceptor,
) -> std::process::Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    // When creating a command with an interceptor, we do not use the double-spawn
    // mechanism. Double-spawn is used to solve races between rapid process
    // creation and SIGTSTP, but with interceptors we're creating processes
    // serially so this is not really a concern.

    let (interceptor_program, interceptor_args) = match interceptor {
        Interceptor::None => unreachable!("create_command_with_interceptor called with None"),
        Interceptor::Debugger(cmd) => (cmd.program(), cmd.args()),
        Interceptor::Tracer(cmd) => (cmd.program(), cmd.args()),
    };

    let mut cmd = std::process::Command::new(interceptor_program);
    cmd.args(interceptor_args);
    cmd.arg(&program);
    cmd.args(args.into_iter().map(|arg| arg.as_ref().to_owned()));

    cmd
}

fn apply_package_env(cmd: &mut std::process::Command, package: &PackageMetadata<'_>) {
    // These environment variables are set at runtime by cargo test:
    // https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
    cmd.env("CARGO_PKG_VERSION", format!("{}", package.version()))
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

/// Applies environment variables spcified by the build script via `cargo::rustc-env`
fn apply_build_script_env(cmd: &mut std::process::Command, out_dir: &Utf8Path) {
    let Some(out_dir_parent) = out_dir.parent() else {
        warn!("could not determine parent directory of output directory {out_dir}");
        return;
    };
    let Ok(out_file) = File::open(out_dir_parent.join("output")) else {
        warn!("could not find build script output file at {out_dir_parent}/output");
        return;
    };
    parse_build_script_output(
        BufReader::new(out_file),
        &out_dir_parent.join("output"),
        |key, val| {
            cmd.env(key, val);
        },
    );
}

/// Parses the build script output and calls the callback for each key value pair of `VAR=val`
///
/// This is mainly split out into a separate function from [`apply_build_script_env`] for easier
/// unit testing
fn parse_build_script_output<R, Cb>(out_file: R, out_file_path: &Utf8Path, mut callback: Cb)
where
    R: BufRead,
    Cb: FnMut(&str, &str),
{
    for line in out_file.lines() {
        let Ok(line) = line else {
            warn!("in build script output `{out_file_path}`, found line with invalid UTF-8");
            continue;
        };
        // `cargo::rustc-env` is the official syntax since `cargo` 1.77, `cargo:rustc-env` is
        // supported for backwards compatibility
        let Some(key_val) = line
            .strip_prefix("cargo::rustc-env=")
            .or_else(|| line.strip_prefix("cargo:rustc-env="))
        else {
            continue;
        };
        let Some((k, v)) = key_val.split_once('=') else {
            warn!("rustc-env variable '{key_val}' has no value in {out_file_path}, skipping");
            continue;
        };
        callback(k, v);
    }
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

    static LD_DYLD_ENV_VARS: LazyLock<HashMap<String, OsString>> = LazyLock::new(|| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    #[test]
    fn parse_build_script() {
        let out_file = indoc! {"
            some_other_line
            cargo::rustc-env=NEW_VAR=new_val
            cargo:rustc-env=OLD_VAR=old_val
            cargo::rustc-env=NEW_MISSING_VALUE
            cargo:rustc-env=OLD_MISSING_VALUE
            cargo:rustc-env=NEW_EMPTY_VALUE=
        "};

        let mut key_vals = Vec::new();
        parse_build_script_output(
            BufReader::new(std::io::Cursor::new(out_file)),
            Utf8Path::new("<test input>"),
            |key, val| key_vals.push((key.to_owned(), val.to_owned())),
        );

        assert_eq!(
            key_vals,
            vec![
                ("NEW_VAR".to_owned(), "new_val".to_owned()),
                ("OLD_VAR".to_owned(), "old_val".to_owned()),
                ("NEW_EMPTY_VALUE".to_owned(), "".to_owned()),
            ],
            "parsed key-value pairs match"
        );
    }
}
