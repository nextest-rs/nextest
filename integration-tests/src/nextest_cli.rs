// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::env::TestEnvInfo;
use camino::Utf8PathBuf;
use color_eyre::{
    Result,
    eyre::{Context, bail, eyre},
};
use nextest_metadata::TestListSummary;
use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsString,
    fmt,
    io::{self, Read, Write},
    iter,
    process::{Command, ExitStatus, Stdio},
};

pub fn cargo_bin() -> String {
    match std::env::var("CARGO") {
        Ok(v) => v,
        Err(std::env::VarError::NotPresent) => "cargo".to_owned(),
        Err(err) => panic!("error obtaining CARGO env var: {err}"),
    }
}

#[derive(Clone, Debug)]
pub struct CargoNextestCli {
    bin: Utf8PathBuf,
    args: Vec<String>,
    envs: HashMap<OsString, OsString>,
    envs_remove: Vec<OsString>,
    current_dir: Option<Utf8PathBuf>,
    unchecked: bool,
}

impl CargoNextestCli {
    pub fn for_test(env_info: &TestEnvInfo) -> Self {
        Self {
            bin: env_info.cargo_nextest_dup_bin.clone(),
            args: vec!["nextest".to_owned(), "--no-pager".to_owned()],
            envs: HashMap::new(),
            envs_remove: Vec::new(),
            current_dir: None,
            unchecked: false,
        }
    }

    /// Creates a new CargoNextestCli instance for use in a setup script.
    ///
    /// Scripts don't have access to the `NEXTEST_BIN_EXE_cargo_nextest_dup` environment variable,
    /// so we run `cargo run --bin cargo-nextest-dup nextest debug current-exe` instead.
    pub fn for_script() -> Result<Self> {
        let cargo_bin = cargo_bin();
        let mut command = std::process::Command::new(&cargo_bin);
        command.args([
            "run",
            "--bin",
            "cargo-nextest-dup",
            "--",
            "nextest",
            "debug",
            "current-exe",
        ]);
        let output = command.output().wrap_err("failed to get current exe")?;

        let output = CargoNextestOutput {
            command: Box::new(command),
            exit_status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        };

        if !output.exit_status.success() {
            bail!("failed to get current exe:\n\n{output:?}");
        }

        // The output is the path to the current exe.
        let exe =
            String::from_utf8(output.stdout).wrap_err("current exe output isn't valid UTF-8")?;

        Ok(Self {
            bin: Utf8PathBuf::from(exe.trim_end()),
            args: vec!["nextest".to_owned()],
            envs: HashMap::new(),
            envs_remove: Vec::new(),
            current_dir: None,
            unchecked: false,
        })
    }

    pub fn arg(&mut self, arg: impl Into<String>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    pub fn args(&mut self, arg: impl IntoIterator<Item = impl Into<String>>) -> &mut Self {
        self.args.extend(arg.into_iter().map(Into::into));
        self
    }

    pub fn env(&mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> &mut Self {
        self.envs.insert(k.into(), v.into());
        self
    }

    pub fn envs(
        &mut self,
        envs: impl IntoIterator<Item = (impl Into<OsString>, impl Into<OsString>)>,
    ) -> &mut Self {
        self.envs
            .extend(envs.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    pub fn env_remove(&mut self, k: impl Into<OsString>) -> &mut Self {
        self.envs_remove.push(k.into());
        self
    }

    pub fn unchecked(&mut self, unchecked: bool) -> &mut Self {
        self.unchecked = unchecked;
        self
    }

    pub fn current_dir(&mut self, dir: impl Into<Utf8PathBuf>) -> &mut Self {
        self.current_dir = Some(dir.into());
        self
    }

    pub fn output(&self) -> CargoNextestOutput {
        let mut command = Command::new(&self.bin);
        command.args(&self.args);
        // Apply env_remove first, then envs, so explicit env() calls can
        // override env_remove().
        for k in &self.envs_remove {
            command.env_remove(k);
        }
        command.envs(&self.envs);
        if let Some(dir) = &self.current_dir {
            command.current_dir(dir);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let command_str = shell_words::join(
            iter::once(self.bin.as_str()).chain(self.args.iter().map(|s| s.as_str())),
        );
        eprintln!("*** executing: {command_str}");

        let mut child = command.spawn().expect("process spawn succeeded");
        let mut stdout = child.stdout.take().expect("stdout is a pipe");
        let mut stderr = child.stderr.take().expect("stderr is a pipe");

        let stdout_thread = std::thread::spawn(move || {
            let mut stdout_buf = Vec::new();
            loop {
                let mut buffer = [0; 1024];
                match stdout.read(&mut buffer) {
                    Ok(n @ 1..) => {
                        stdout_buf.extend_from_slice(&buffer[..n]);
                        let mut io_stdout = std::io::stdout().lock();
                        io_stdout
                            .write_all(&buffer[..n])
                            .wrap_err("error writing to our stdout")?;
                        io_stdout.flush().wrap_err("error flushing our stdout")?;
                    }
                    Ok(0) => break Ok(stdout_buf),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        break Err(eyre!(error).wrap_err("error reading from child stdout"));
                    }
                }
            }
        });

        let stderr_thread = std::thread::spawn(move || {
            let mut stderr_buf = Vec::new();
            loop {
                let mut buffer = [0; 1024];
                match stderr.read(&mut buffer) {
                    Ok(n @ 1..) => {
                        stderr_buf.extend_from_slice(&buffer[..n]);
                        let mut io_stderr = std::io::stderr().lock();
                        io_stderr
                            .write_all(&buffer[..n])
                            .wrap_err("error writing to our stderr")?;
                        io_stderr.flush().wrap_err("error flushing our stderr")?;
                    }
                    Ok(0) => break Ok(stderr_buf),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        break Err(eyre!(error).wrap_err("error reading from child stderr"));
                    }
                }
            }
        });

        // Wait for the child process to finish first. The stdout and stderr
        // threads will exit once the process has exited and the pipes' write
        // ends have been closed.
        let exit_status = child.wait().expect("child process exited");

        let stdout_buf = stdout_thread
            .join()
            .expect("stdout thread exited without panicking")
            .expect("wrote to our stdout successfully");
        let stderr_buf = stderr_thread
            .join()
            .expect("stderr thread exited without panicking")
            .expect("wrote to our stderr successfully");

        let ret = CargoNextestOutput {
            command: Box::new(command),
            exit_status,
            stdout: stdout_buf,
            stderr: stderr_buf,
        };

        eprintln!("*** command {command_str} exited with status {exit_status}");

        if !self.unchecked && !exit_status.success() {
            panic!("command failed");
        }

        ret
    }
}

pub struct CargoNextestOutput {
    pub command: Box<Command>,
    pub exit_status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CargoNextestOutput {
    pub fn stdout_as_str(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }

    pub fn stderr_as_str(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stderr)
    }

    pub fn decode_test_list_json(&self) -> Result<TestListSummary> {
        Ok(serde_json::from_slice(&self.stdout)?)
    }

    /// Returns the output as a (hopefully) platform-independent snapshot that
    /// can be checked in and compared.
    pub fn to_snapshot(&self) -> String {
        // Don't include the command as its representation is
        // platform-dependent.
        let output = format!(
            "exit code: {:?}\n\
            --- stdout ---\n{}\n\n--- stderr ---\n{}\n",
            self.exit_status.code(),
            String::from_utf8_lossy(&self.stdout),
            String::from_utf8_lossy(&self.stderr),
        );

        // Turn "exit status" and "exit code" into "exit status|code"
        let output = output.replace("exit status: ", "exit status|code: ");
        output.replace("exit code: ", "exit status|code: ")
    }
}

impl fmt::Display for CargoNextestOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "command: {:?}\nexit code: {:?}\n\
                   --- stdout ---\n{}\n\n--- stderr ---\n{}\n\n",
            self.command,
            self.exit_status.code(),
            String::from_utf8_lossy(&self.stdout),
            String::from_utf8_lossy(&self.stderr)
        )
    }
}

// Make Debug output the same as Display output, so `.unwrap()` and `.expect()` are nicer.
impl fmt::Debug for CargoNextestOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
