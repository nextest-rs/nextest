// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Regenerates the Rust bindings for Buck2's test executor protocol.
//!
//! The generated code is committed rather than produced by a build script, so
//! that building the workspace does not require `protoc`. Run this after
//! resyncing `proto/test.proto` with Buck2:
//!
//! ```console
//! $ just generate-proto
//! ```

use camino::Utf8Path;
use miette::{Context, IntoDiagnostic, Result};
use std::fs;

/// The header prepended to the generated file.
///
/// The generator runs the output through `rustfmt`, so the committed file is
/// already formatted and `cargo xfmt` leaves it alone. The lint allows cover
/// what machine-generated code does not bother with.
const HEADER: &str = "\
// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Generated bindings for Buck2's test executor protocol.
//!
//! Do not edit by hand. Regenerate with `just generate-proto` after changing
//! `buck2-nextest/proto/test.proto`, which documents where the protocol comes
//! from.

#![allow(clippy::all, missing_docs, unreachable_pub)]

";

fn main() -> Result<()> {
    let manifest_dir = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
    let proto_dir = manifest_dir.join("proto");
    let proto_file = proto_dir.join("test.proto");
    let out_dir = manifest_dir.join("src/proto");

    // The generator names its output after the proto package, so it lands at
    // `buck.test.rs`; the header is prepended and the result moved into place
    // under a name that reads as Rust rather than as a proto package.
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .out_dir(out_dir.as_std_path())
        .compile_protos(&[proto_file.as_str()], &[proto_dir.as_str()])
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to compile {proto_file}"))?;

    let raw = out_dir.join("buck.test.rs");
    let contents = fs::read_to_string(&raw)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read generated code from {raw}"))?;

    let dest = out_dir.join("generated.rs");
    fs::write(&dest, format!("{HEADER}{contents}"))
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to write generated code to {dest}"))?;

    fs::remove_file(&raw)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to remove {raw}"))?;

    println!("{dest}");
    Ok(())
}
