// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use buck2_nextest::cli::App;
use clap::Parser;
use nextest_runner::write_str::WriteStr;

fn main() -> std::process::ExitCode {
    let cli_args: Vec<String> = std::env::args().collect();
    let app = App::parse();

    let mut stdout = std::io::stdout();
    let mut writer = StdoutWriter(&mut stdout);

    match app.exec(&mut writer, cli_args) {
        Ok(code) => std::process::ExitCode::from(code as u8),
        Err(error) => {
            let code = error.exit_code();
            // Filterset diagnostics carry their own source text and are
            // rendered separately.
            error.display_extra();
            eprintln!("{:?}", miette::Report::new(error));
            std::process::ExitCode::from(code as u8)
        }
    }
}

struct StdoutWriter<'a>(&'a mut std::io::Stdout);

impl WriteStr for StdoutWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::io::Result<()> {
        use std::io::Write;
        self.0.write_all(s.as_bytes())
    }

    fn write_str_flush(&mut self) -> std::io::Result<()> {
        use std::io::Write;
        self.0.flush()
    }
}
