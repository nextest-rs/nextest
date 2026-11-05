// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_config::EnvironmentMap,
    config::scripts::ScriptCommandEnvMap,
    double_spawn::{DoubleSpawnContext, DoubleSpawnInfo},
    helpers::dylib_path_envvar,
    list::{RustBuildMeta, TestListState},
    runner::{Interceptor, VersionEnvVars},
    test_output::CaptureStrategy,
};
use camino::{Utf8Path, Utf8PathBuf};
use guppy::graph::PackageMetadata;
use quick_junit::ReportUuid;
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    ffi::{OsStr, OsString},
    fs::File,
    io::{BufRead, BufReader, PipeReader, PipeWriter},
    sync::{LazyLock, PoisonError, RwLock},
};
use tracing::warn;

mod imp;
use imp::attach_capture_readers;
#[cfg(test)]
pub(crate) use imp::pipe_reader_to_child_stdout;
pub(crate) use imp::{Child, ChildAccumulator, ChildFds};

/// Serializes pipe creation against process spawning.
///
/// On Unix, `fork` copies every descriptor open in the parent into the child,
/// and `FD_CLOEXEC` only closes those copies at the child's `execve`. A child
/// that has been forked but not yet scheduled to exec therefore holds a copy of
/// every descriptor in this process -- including the capture-pipe write ends
/// created for a *different* test. Under load the fork-to-`execve` gap can
/// exceed `leak-timeout`, and the other test is reported as leaky even though
/// it spawned nothing and holds no descriptor. (rust-lang/rust#95584 is the
/// non-atomic-CLOEXEC variant of the same exposure; atomic CLOEXEC only closes
/// the window inside `std::io::pipe()`, not the inheritance that lasts until
/// the new child execs.)
///
/// The window is closed by keeping pipe creation and the fork indivisible:
///
/// * a spawn that creates capture pipes holds the write lock across
///   `create_pipe` -> spawn -> the drop of this process's copies of the write
///   ends, which happens inside [`spawn_process`] when the temporary
///   `tokio::process::Command` is dropped;
/// * a spawn that creates no pipes holds the read lock across its fork, so it
///   cannot land inside such a window.
///
/// This is required on every Unix target, not only those without atomic
/// CLOEXEC. Windows needs no lock: there is no `fork`, and std already
/// serializes `CreateProcess`.
#[cfg(unix)]
static PROCESS_SPAWN_LOCK: RwLock<()> = RwLock::new(());

/// Which side of [`PROCESS_SPAWN_LOCK`] a spawn takes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SpawnLockMode {
    /// The spawn creates capture pipes: hold the write lock across pipe
    /// creation, the fork, and the drop of this process's write-end copies.
    Captured,
    /// The spawn creates no pipes: hold the read lock across its fork.
    Plain,
}

/// Holds [`PROCESS_SPAWN_LOCK`] for the duration of a spawn.
#[derive(Debug)]
pub(crate) struct ProcessSpawnGuard {
    #[cfg(unix)]
    _inner: SpawnLockInner,
}

#[cfg(unix)]
#[derive(Debug)]
// The guards are held only for their `Drop` implementations and are never
// read, so dead-code analysis would otherwise flag both payloads.
#[allow(dead_code)]
enum SpawnLockInner {
    Write(std::sync::RwLockWriteGuard<'static, ()>),
    Read(std::sync::RwLockReadGuard<'static, ()>),
}

/// Acquires the process-spawn lock in the mode appropriate for a spawn.
///
/// A [`SpawnLockMode::Captured`] guard must be held from before [`create_pipe`]
/// until after [`spawn_process`] returns; a [`SpawnLockMode::Plain`] guard must
/// be held across [`spawn_process`].
pub(crate) fn lock_process_spawn(mode: SpawnLockMode) -> ProcessSpawnGuard {
    #[cfg(unix)]
    {
        // This lock protects no data, so a panic leaves no state to repair.
        let inner = match mode {
            SpawnLockMode::Captured => SpawnLockInner::Write(
                PROCESS_SPAWN_LOCK
                    .write()
                    .unwrap_or_else(PoisonError::into_inner),
            ),
            SpawnLockMode::Plain => SpawnLockInner::Read(
                PROCESS_SPAWN_LOCK
                    .read()
                    .unwrap_or_else(PoisonError::into_inner),
            ),
        };
        ProcessSpawnGuard { _inner: inner }
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        ProcessSpawnGuard {}
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LocalExecuteContext<'a> {
    pub(crate) phase: TestCommandPhase,
    pub(crate) run_id: ReportUuid,
    pub(crate) version_env_vars: &'a VersionEnvVars,
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
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        lctx: &LocalExecuteContext<'_>,
        program: String,
        args: &[Cow<'_, str>],
        env: Option<&ScriptCommandEnvMap>,
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

        // Apply Cargo's config.toml env first (workspace-wide), then the
        // wrapper's command.env (per-script). This way command.env takes
        // priority as the more specific configuration.
        lctx.env.apply_env(&mut cmd);
        if let Some(env) = env {
            env.apply_env(&mut cmd);
        }

        if let Some(out_dir) = lctx
            .rust_build_meta
            .build_script_out_dirs
            .get(package.id().repr())
        {
            // Convert the output directory to an absolute path. Build script
            // out_dirs are relative to the build directory.
            let out_dir = lctx.rust_build_meta.build_directory.join(out_dir);
            cmd.env("OUT_DIR", &out_dir);

            // Apply build script rustc-env variables. If build_script_info is
            // present (true for new archives and fresh builds), use the
            // structured data captured from the Cargo message. If this is None,
            // this is an old archive that predates the field: fall back to
            // parsing the raw output file.
            //
            // Note that using build script environment variables at runtime is
            // supported by cargo test, but discouraged.
            match &lctx.rust_build_meta.build_script_info {
                Some(info) => {
                    if let Some(info) = info.get(package.id().repr()) {
                        for (key, val) in &info.envs {
                            cmd.env(key, val);
                        }
                    }
                }
                None => {
                    apply_build_script_env(&mut cmd, &out_dir);
                }
            }
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
        // work.
        //
        // CARGO_BIN_EXE_<name> is set by Cargo at build time, and as of Rust
        // 1.94, also at runtime. Set it here (with the hyphen version) to match
        // Cargo's behavior.
        //
        // NEXTEST_BIN_EXE_<name> is additionally set for both the hyphen and
        // underscore variants. Some shells and debuggers drop environment
        // variables with hyphens in their names, so the underscore variant is
        // provided as a workaround.
        //
        // See
        // https://unix.stackexchange.com/questions/23659/can-shell-variable-name-include-a-hyphen-or-dash.
        for (name, path) in non_test_binaries {
            cmd.env(format!("CARGO_BIN_EXE_{name}"), path);
            cmd.env(format!("NEXTEST_BIN_EXE_{name}"), path);
            let with_underscores = name.replace('-', "_");
            if &with_underscores != name {
                cmd.env(format!("NEXTEST_BIN_EXE_{with_underscores}"), path);
            }
        }

        let double_spawn = lctx.double_spawn.spawn_context();

        Self {
            program,
            args: args.iter().map(|arg| arg.to_string()).collect(),
            command: cmd,
            double_spawn,
        }
    }

    pub(crate) fn program(&self) -> &str {
        &self.program
    }

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
        let res = spawn_piped(self.command, true, true);

        if let Some(ctx) = self.double_spawn {
            ctx.finish();
        }

        res?.wait_with_output().await
    }
}

/// Use std's pipes for atomic CLOEXEC on illumos (missing in mio-pipe 0.1.1)
/// and anonymous pipes on Windows, which Tokio does not provide.
///
/// A caller that creates a capture pipe must hold a
/// [`SpawnLockMode::Captured`] guard (see [`lock_process_spawn`]) across this
/// call and the spawn.
fn create_pipe() -> std::io::Result<(PipeReader, PipeWriter)> {
    std::io::pipe()
}

/// Capture pipes must come from `create_pipe`, not `Stdio::piped()`.
///
/// The caller must hold the process-spawn lock (see [`lock_process_spawn`]).
/// This process's copies of any capture write ends stored in `cmd` are closed
/// when the temporary `tokio::process::Command` below is dropped, i.e. before
/// this function returns and while the caller still holds the guard.
fn spawn_process(cmd: std::process::Command) -> std::io::Result<tokio::process::Child> {
    tokio::process::Command::from(cmd).spawn()
}

pub(crate) fn spawn_piped(
    mut cmd: std::process::Command,
    capture_stdout: bool,
    capture_stderr: bool,
) -> std::io::Result<tokio::process::Child> {
    // No other thread may fork while these pipes are in the descriptor table;
    // see `PROCESS_SPAWN_LOCK`. The guard must outlive `spawn_process`, whose
    // dropped `Command` closes this process's copies of the write ends.
    let spawn_guard = lock_process_spawn(if capture_stdout || capture_stderr {
        SpawnLockMode::Captured
    } else {
        SpawnLockMode::Plain
    });

    let stdout_rx = if capture_stdout {
        let (rx, tx) = create_pipe()?;
        cmd.stdout(tx);
        Some(rx)
    } else {
        None
    };
    let stderr_rx = if capture_stderr {
        let (rx, tx) = create_pipe()?;
        cmd.stderr(tx);
        Some(rx)
    } else {
        None
    };

    // Test-only rendezvous inside the critical section; see `test_hooks`.
    #[cfg(all(test, unix))]
    test_hooks::park_if_hooked(cmd.get_program());

    let mut child = spawn_process(cmd)?;
    drop(spawn_guard);
    if let Err(error) = attach_capture_readers(&mut child, stdout_rx, stderr_rx) {
        // The caller cannot supervise a child it never receives.
        _ = child.start_kill();
        return Err(error);
    }
    Ok(child)
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
    cmd.env("CARGO_PKG_VERSION", package.version().to_string())
        .env(
            "CARGO_PKG_VERSION_MAJOR",
            package.version().major.to_string(),
        )
        .env(
            "CARGO_PKG_VERSION_MINOR",
            package.version().minor.to_string(),
        )
        .env(
            "CARGO_PKG_VERSION_PATCH",
            package.version().patch.to_string(),
        )
        .env("CARGO_PKG_VERSION_PRE", package.version().pre.to_string())
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
                .rust_version()
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
            cmd.env(format!("NEXTEST_{k}"), v);
        }
    }
    // Also add the dylib path envvar under the NEXTEST_ prefix.
    if is_sip_sanitized(dylib_path_envvar()) {
        cmd.env(format!("NEXTEST_{}", dylib_path_envvar()), dylib_path);
    }
}

/// Test-only rendezvous inside the spawn lock's critical section.
///
/// The window between pipe creation and the drop of this process's write-end
/// copies is only a few syscalls wide, so a regression test cannot reliably
/// land a competing fork inside it by racing. The test installs a hook for a
/// unique program path; `spawn_piped` parks that one spawn inside the window
/// (while it holds the write lock) until the test has attempted the competing
/// fork.
#[cfg(all(test, unix))]
pub(crate) mod test_hooks {
    use std::{
        ffi::OsStr,
        path::{Path, PathBuf},
        sync::{Arc, Mutex, mpsc},
    };

    struct SpawnHook {
        program: PathBuf,
        reached: mpsc::Sender<()>,
        resume: Arc<Mutex<mpsc::Receiver<()>>>,
    }

    static HOOK: Mutex<Option<SpawnHook>> = Mutex::new(None);

    /// Installs a hook for `program`. A matching spawn signals `reached` and
    /// then blocks until a message arrives on `resume`.
    pub(crate) fn install(
        program: &Path,
        reached: mpsc::Sender<()>,
        resume: Arc<Mutex<mpsc::Receiver<()>>>,
    ) {
        *HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(SpawnHook {
            program: program.to_owned(),
            reached,
            resume,
        });
    }

    /// Removes the hook.
    pub(crate) fn clear() {
        *HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    pub(super) fn park_if_hooked(program: &OsStr) {
        let (reached, resume) = {
            let hook = HOOK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match hook.as_ref() {
                Some(hook) if hook.program.as_os_str() == program => {
                    (hook.reached.clone(), Arc::clone(&hook.resume))
                }
                _ => return,
            }
        };
        // The hook mutex is not held while parked; other spawns still serialize
        // on the spawn lock, which is the point.
        let _ = reached.send(());
        let _ = resume
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recv();
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

#[cfg(all(test, unix))]
mod spawn_lock_tests {
    use super::*;
    use std::{
        os::{
            raw::{c_char, c_int},
            unix::fs::symlink,
        },
        sync::{Arc, Mutex, mpsc},
        time::{Duration, Instant},
    };
    use tokio::io::AsyncReadExt;

    /// Far beyond nextest's 200 ms default leak timeout: the sibling child
    /// holds every descriptor inherited at `fork` until it execs, so the
    /// elapsed hold must overshoot the budget whenever the leak check starts.
    const SIBLING_EXEC_DELAY: Duration = Duration::from_secs(2);
    const LEAK_TIMEOUT: Duration = Duration::from_millis(200);

    unsafe extern "C" {
        fn fork() -> c_int;
        fn usleep(usec: u32) -> c_int;
        fn execv(path: *const c_char, argv: *const *const c_char) -> c_int;
        fn _exit(code: c_int) -> !;
        fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    }

    enum Outcome {
        NoLeak { time_to_close: Duration },
        Leaked,
    }

    struct HookCleanup;

    impl Drop for HookCleanup {
        fn drop(&mut self) {
            test_hooks::clear();
        }
    }

    /// Forks and delays the child's `execve`, modelling the runner's
    /// fork-to-`execve` gap under load. The child holds every descriptor
    /// inherited at `fork` until it execs.
    fn fork_and_exec_after(delay: Duration) {
        let pid = unsafe { fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            let path = c"/bin/true";
            let argv = [path.as_ptr(), std::ptr::null()];
            unsafe {
                usleep(delay.as_micros() as u32);
                execv(path.as_ptr(), argv.as_ptr());
                _exit(127);
            }
        }
        let mut status = 0;
        let reaped = unsafe { waitpid(pid, &mut status, 0) };
        assert_eq!(reaped, pid, "sibling reaped");
    }

    async fn wait_for_eof(
        mut stdout: tokio::process::ChildStdout,
        mut stderr: tokio::process::ChildStderr,
    ) -> Outcome {
        let started = Instant::now();
        let mut obuf = Vec::new();
        let mut ebuf = Vec::new();
        let read = async {
            let (out, err) =
                tokio::join!(stdout.read_to_end(&mut obuf), stderr.read_to_end(&mut ebuf));
            out.expect("stdout read");
            err.expect("stderr read");
        };
        match tokio::time::timeout(LEAK_TIMEOUT, read).await {
            Ok(()) => Outcome::NoLeak {
                time_to_close: started.elapsed(),
            },
            Err(_) => Outcome::Leaked,
        }
    }

    /// A test that spawns nothing and is fully reaped must not be reported
    /// leaky because a *different* spawn forked while its capture pipes were in
    /// this process's descriptor table and that child had not exec'd yet.
    ///
    /// Without the cross-spawn lock this fails: the sibling forked inside the
    /// window holds the victim's capture pipes for `SIBLING_EXEC_DELAY`, so the
    /// victim's reads do not reach EOF within `LEAK_TIMEOUT`.
    #[test]
    fn capture_pipes_are_not_inherited_by_a_concurrent_spawn() {
        let dir =
            std::env::temp_dir().join(format!("nextest-runner-spawn-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let victim_program = dir.join("victim-pure");
        let _ = std::fs::remove_file(&victim_program);
        symlink("/bin/true", &victim_program).expect("victim symlink");

        let (reached_tx, reached_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        test_hooks::install(&victim_program, reached_tx, Arc::new(Mutex::new(resume_rx)));
        let _cleanup = HookCleanup;

        let program = victim_program.clone();
        let victim = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            rt.block_on(async move {
                let cmd = std::process::Command::new(&program);
                let mut child = spawn_piped(cmd, true, true).expect("victim spawns");
                let stdout = child.stdout.take().expect("stdout captured");
                let stderr = child.stderr.take().expect("stderr captured");
                child.wait().await.expect("victim reaped");
                wait_for_eof(stdout, stderr).await
            })
        });

        // The victim's pipes now exist and are in this process's descriptor
        // table; the victim's spawn is parked before `spawn_process`.
        reached_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("victim reaches the spawn window");

        // A competing spawn takes the read side of the spawn lock, exactly as a
        // pipe-less runner spawn would. Without the fix it forks here, inside
        // the victim's window.
        let (about_to_fork_tx, about_to_fork_rx) = mpsc::channel();
        let sibling = std::thread::spawn(move || {
            about_to_fork_tx.send(()).expect("signal");
            let _guard = lock_process_spawn(SpawnLockMode::Plain);
            fork_and_exec_after(SIBLING_EXEC_DELAY);
        });
        about_to_fork_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("sibling reaches the spawn lock");
        // Give the sibling time to fork (without the fix) or block on the lock
        // (with it). The victim is parked, so no leak budget elapses.
        std::thread::sleep(Duration::from_millis(500));
        resume_tx.send(()).expect("resume the victim");

        let outcome = victim.join().expect("victim thread");
        sibling.join().expect("sibling thread");

        match outcome {
            Outcome::NoLeak { time_to_close } => {
                // Informational: carry the measurement, not just the verdict.
                eprintln!("victim's capture pipes closed after {time_to_close:?}");
            }
            Outcome::Leaked => panic!(
                "the pure victim was reported leaky: a concurrently spawned sibling held its \
                 capture pipe until the sibling exec'd. The spawn lock must close that window."
            ),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
