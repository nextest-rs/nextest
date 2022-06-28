// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{AppOpts, InstallManError};
use camino::{Utf8Path, Utf8PathBuf};
use clap::CommandFactory;
use clap_mangen::Man;

pub(crate) fn install_man(output_dir: Option<Utf8PathBuf>) -> Result<(), InstallManError> {
    let mut output_dir = match output_dir {
        Some(d) => d,
        None => {
            let mut current_exe = std::env::current_exe()
                .and_then(|home| {
                    Utf8PathBuf::try_from(home).map_err(|error| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, error)
                    })
                })
                .map_err(|error| InstallManError::CurrentExe { error })?;
            // If the current exe is foo/bar/bin/cargo-nextest, the man directory is foo/bar/man.
            current_exe.pop();
            current_exe.pop();
            current_exe.push("man");
            current_exe
        }
    };

    // All of nextest's commands go in man1.
    output_dir.push("man1");

    std::fs::create_dir_all(&output_dir).map_err(|error| InstallManError::CreateOutputDir {
        path: output_dir.clone(),
        error,
    })?;

    let command = AppOpts::command();

    let man = Man::new(command.clone()).manual("Nextest Manual");
    let path = output_dir.join("cargo-nextest.1");
    render_to_file(&man, &path).map_err(|error| InstallManError::WriteToFile { path, error })?;

    for subcommand in command.get_subcommands() {
        let name = subcommand.get_name();
        // XXX this line crashes with "Command list: Argument or group 'manifest-path' specified in
        // 'conflicts_with*' for 'cargo-metadata' does not exist".
        let man = Man::new(subcommand.clone()).manual("Nextest Manual");
        let path = output_dir.join(format!("cargo-nextest-{}.1", name));
        render_to_file(&man, &path)
            .map_err(|error| InstallManError::WriteToFile { path, error })?;
    }

    Ok(())
}

fn render_to_file(man: &Man, path: &Utf8Path) -> Result<(), std::io::Error> {
    let mut writer = std::fs::File::create(&path)?;
    man.render(&mut writer)
}
