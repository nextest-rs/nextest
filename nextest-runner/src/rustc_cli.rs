// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::cargo_config::TargetTriple;
use camino::Utf8PathBuf;
use std::{borrow::Cow, path::PathBuf};
use tracing::{debug, trace};

/// Create a rustc CLI call.
#[derive(Clone, Debug)]
pub struct RustcCli<'a> {
    rustc_path: Utf8PathBuf,
    args: Vec<Cow<'a, str>>,
}

impl<'a> RustcCli<'a> {
    /// Create a rustc CLI call: `rustc --version --verbose`.
    pub fn version_verbose() -> Self {
        let mut cli = Self::default();
        cli.add_arg("--version").add_arg("--verbose");
        cli
    }

    /// Create a rustc CLI call: `rustc --print target-libdir`.
    pub fn print_host_libdir() -> Self {
        let mut cli = Self::default();
        cli.add_arg("--print").add_arg("target-libdir");
        cli
    }

    /// Create a rustc CLI call: `rustc --print target-libdir --target <triple>`.
    pub fn print_target_libdir(triple: &'a TargetTriple) -> Self {
        let mut cli = Self::default();
        cli.add_arg("--print")
            .add_arg("target-libdir")
            .add_arg("--target")
            .add_arg(triple.platform.triple_str());
        cli
    }

    fn add_arg(&mut self, arg: impl Into<Cow<'a, str>>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    /// Convert the command to a [`duct::Expression`].
    pub fn to_expression(&self) -> duct::Expression {
        duct::cmd(self.rustc_path.as_str(), self.args.iter().map(|arg| &**arg))
    }

    /// Execute the command, capture its standard output, and return the captured output as a
    /// [`Vec<u8>`].
    pub fn read(&self) -> Option<Vec<u8>> {
        let expression = self.to_expression();
        trace!("Executing command: {:?}", expression);
        let output = match expression
            .stdout_capture()
            .stderr_capture()
            .unchecked()
            .run()
        {
            Ok(output) => output,
            Err(e) => {
                debug!("Failed to spawn the child process: {}", e);
                return None;
            }
        };
        if !output.status.success() {
            debug!("execution failed with {}", output.status);
            debug!("stdout:");
            debug!("{}", String::from_utf8_lossy(&output.stdout));
            debug!("stderr:");
            debug!("{}", String::from_utf8_lossy(&output.stderr));
            return None;
        }
        Some(output.stdout)
    }
}

impl Default for RustcCli<'_> {
    fn default() -> Self {
        Self {
            rustc_path: rustc_path(),
            args: vec![],
        }
    }
}

fn rustc_path() -> Utf8PathBuf {
    match std::env::var_os("RUSTC") {
        Some(rustc_path) => PathBuf::from(rustc_path)
            .try_into()
            .expect("RUSTC env var is not valid UTF-8"),
        None => Utf8PathBuf::from("rustc"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino_tempfile::Utf8TempDir;
    use std::env;

    #[test]
    fn test_should_run_rustc_version() {
        let mut cli = RustcCli::default();
        cli.add_arg("--version");
        let output = cli.read().expect("rustc --version should run successfully");
        let output = String::from_utf8(output).expect("the output should be valid utf-8");
        assert!(
            output.starts_with("rustc"),
            "The output should start with rustc, but the actual output is: {output}"
        );
    }

    #[test]
    fn test_should_respect_rustc_env() {
        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe { env::set_var("RUSTC", "cargo") };
        let mut cli = RustcCli::default();
        cli.add_arg("--version");
        let output = cli.read().expect("cargo --version should run successfully");
        let output = String::from_utf8(output).expect("the output should be valid utf-8");
        assert!(
            output.starts_with("cargo"),
            "The output should start with cargo, but the actual output is: {output}"
        );
    }

    #[test]
    fn test_fail_to_spawn() {
        let fake_dir = Utf8TempDir::new().expect("should create the temp dir successfully");
        // No OS will allow executing a directory.
        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe { env::set_var("RUSTC", fake_dir.path()) };
        let mut cli = RustcCli::default();
        cli.add_arg("--version");
        let output = cli.read();
        assert_eq!(output, None);
    }

    #[test]
    fn test_execute_with_failure() {
        let mut cli = RustcCli::default();
        // rustc --print Y7uDG1HrrY should fail
        cli.add_arg("--print");
        cli.add_arg("Y7uDG1HrrY");
        let output = cli.read();
        assert_eq!(output, None);
    }
}
