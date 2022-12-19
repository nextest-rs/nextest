//! A small passthrough script used for testing. We can't use bash here because of System Integrity
//! Protection on macOS: <https://github.com/nextest-rs/nextest/pull/84#issuecomment-1057287763>

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    process::{exit, Command},
};

fn main() {
    let args: Vec<String> = env::args().collect();
    eprintln!("[passthrough] args: {args:?}");

    if args[1] != "--ensure-this-arg-is-sent" {
        eprintln!("[passthrough] --ensure-this-arg-is-sent not passed as the first element");
        exit(1);
    }

    // Ensure that LD_ and DYLD_ env vars are also prefixed with NEXTEST_ as a workaround
    // for macOS SIP.
    let ld_dyld_paths = sanitized_env_vars("");
    let nextest_ld_dyld_paths = sanitized_env_vars("NEXTEST_");

    assert_eq!(
        ld_dyld_paths, nextest_ld_dyld_paths,
        "[passthrough] SIP-sanitized env vars should be identical under the NEXTEST_ prefix"
    );

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

// This is a BTreeMap for better error messages.
fn sanitized_env_vars(strip_prefix: &str) -> BTreeMap<String, OsString> {
    std::env::vars_os()
        .filter_map(|(k, v)| match k.into_string() {
            Ok(k) => k
                // first strip the prefix...
                .strip_prefix(strip_prefix)
                // ...then check whether the suffix is SIP-sanitized
                .and_then(|suffix| is_sip_sanitized(suffix).then(|| (suffix.to_owned(), v))),
            Err(_) => None,
        })
        .collect()
}

fn is_sip_sanitized(var: &str) -> bool {
    var.starts_with("LD_") || var.starts_with("DYLD_")
}
