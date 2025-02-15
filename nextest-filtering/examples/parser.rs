// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Standalone expression parser
//!
//! Useful for manually testing the parsing result

use camino::Utf8PathBuf;
use clap::Parser;
use guppy::graph::PackageGraph;
use nextest_filtering::errors::FiltersetParseErrors;

#[derive(Debug, Parser)]
struct Args {
    /// Path to a cargo metadata json file
    #[clap(short = 'g')]
    cargo_metadata: Option<Utf8PathBuf>,

    /// The expression to parse
    expr: String,
}

const EMPTY_GRAPH: &str = r#"{
        "packages": [],
        "workspace_members": [],
        "workspace_root": "",
        "target_directory": "",
        "version": 1
    }"#;

fn load_graph(path: Option<Utf8PathBuf>) -> PackageGraph {
    let json = match path {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(json) => json,
            Err(err) => {
                eprintln!("Failed to read cargo-metadata file: {err}");
                std::process::exit(1);
            }
        },
        None => EMPTY_GRAPH.to_string(),
    };

    match PackageGraph::from_json(json) {
        Ok(graph) => graph,
        Err(err) => {
            eprintln!("Failed to parse cargo-metadata: {err}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args = Args::parse();

    let graph = load_graph(args.cargo_metadata);
    let cx = nextest_filtering::ParseContext::new(&graph);
    match nextest_filtering::Filterset::parse(
        args.expr,
        &cx,
        nextest_filtering::FiltersetKind::Test,
    ) {
        Ok(expr) => println!("{expr:?}"),
        Err(FiltersetParseErrors { input, errors, .. }) => {
            for error in errors {
                let report = miette::Report::new(error).with_source_code(input.clone());
                eprintln!("{report:?}");
            }
            std::process::exit(1);
        }
    }
}
