// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8PathBuf;
use color_eyre::{
    Result,
    eyre::{Context, bail},
};
use nextest_metadata::TestListSummary;
use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsString,
    fmt,
    process::{Command, ExitStatus},
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
    unchecked: bool,
}

impl CargoNextestCli {
    pub fn for_test() -> Self {
        let bin = std::env::var("NEXTEST_BIN_EXE_cargo-nextest-dup")
            .expect("unable to find cargo-nextest-dup");
        Self {
            bin: bin.into(),
            args: vec!["nextest".to_owned()],
            envs: HashMap::new(),
            unchecked: false,
        }
    }

    /// Creates a new CargoNextestCli instance for use in a setup script.
    ///
    /// Scripts don't have access to the `NEXTEST_BIN_EXE_cargo-nextest-dup` environment variable,
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
            command,
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

    pub fn unchecked(&mut self, unchecked: bool) -> &mut Self {
        self.unchecked = unchecked;
        self
    }

    pub fn output(&self) -> CargoNextestOutput {
        let mut command = std::process::Command::new(&self.bin);
        command.args(&self.args);
        command.envs(&self.envs);
        let output = command.output().expect("failed to execute");

        let ret = CargoNextestOutput {
            command,
            exit_status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        };

        if !self.unchecked && !output.status.success() {
            panic!("command failed:\n\n{ret}");
        }

        ret
    }
}

pub struct CargoNextestOutput {
    pub command: Command,
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
