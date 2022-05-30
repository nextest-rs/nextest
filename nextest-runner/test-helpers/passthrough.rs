//! A small passthrough script used for testing. We can't use bash here because of System Integrity
//! Protection on macOS: <https://github.com/nextest-rs/nextest/pull/84#issuecomment-1057287763>

use std::{
    env,
    process::{exit, Command},
};

fn main() {
    let args: Vec<String> = env::args().collect();
    eprintln!("[passthrough] args: {:?}", args);

    if args[1] != "--ensure-this-arg-is-sent" {
        eprintln!("[passthrough] --ensure-this-arg-is-sent not passed as the first element");
        exit(1);
    }

    match Command::new(&args[2]).args(&args[3..]).status() {
        Ok(status) => match status.code() {
            Some(code) => {
                exit(code);
            }
            None => {
                eprintln!("[passthrough] process did not exit with a code, exiting with 101");
                exit(101);
            }
        },
        Err(err) => {
            eprintln!(
                "[passthrough] failed to spawn subprocess '{}': {}, exiting with 102",
                args[2], err
            );
            exit(102);
        }
    }
}
