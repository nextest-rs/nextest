// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::test_list::TestBinary;
use cargo_metadata::Message;
use color_eyre::eyre::{Result, WrapErr};
use guppy::{graph::PackageGraph, PackageId};
use std::io::BufRead;

impl TestBinary {
    /// Parse Cargo messages from the given `BufRead` and return a list of test binaries.
    pub fn from_messages(graph: &PackageGraph, reader: impl BufRead) -> Result<Vec<Self>> {
        let mut binaries = vec![];

        for message in Message::parse_stream(reader) {
            let message = message.wrap_err("failed to read Cargo JSON message")?;
            match message {
                Message::CompilerArtifact(artifact) if artifact.profile.test => {
                    if let Some(binary) = artifact.executable {
                        // Look up the executable by package ID.
                        let package_id = PackageId::new(artifact.package_id.repr);
                        let package = graph
                            .metadata(&package_id)
                            .wrap_err("package ID not found in package graph")?;

                        // Tests are run in the directory containing Cargo.toml
                        let cwd = package
                            .manifest_path()
                            .parent()
                            .unwrap_or_else(|| {
                                panic!(
                                    "manifest path {} doesn't have a parent",
                                    package.manifest_path()
                                )
                            })
                            .to_path_buf();

                        // Construct the binary ID from the package and build target.
                        let mut binary_id = package.name().to_owned();
                        if artifact.target.name != package.name() {
                            binary_id.push_str("::");
                            binary_id.push_str(&artifact.target.name);
                        }

                        binaries.push(TestBinary {
                            binary,
                            binary_id,
                            cwd: Some(cwd),
                        })
                    }
                }
                _ => {
                    // Ignore all other messages.
                }
            }
        }

        Ok(binaries)
    }
}
