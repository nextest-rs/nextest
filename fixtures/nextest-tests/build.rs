//! Adapted from
//! https://github.com/dtolnay/syn/blob/a54fb0098c6679f1312113ae2eec0305c51c7390/build.rs.

// TODO: remove once MSRV is 1.64

fn main() {
    let version = rustc_minor_version().expect("unable to determine Rust version");
    if version < 64 {
        println!("cargo:rustc-cfg=no_pkg_rust_version");
    }
}

fn rustc_minor_version() -> Option<u32> {
    let rustc = std::env::var_os("RUSTC")?;
    let output = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()?;
    let version = std::str::from_utf8(&output.stdout).ok()?;
    let mut pieces = version.split('.');
    if pieces.next() != Some("rustc 1") {
        return None;
    }
    let minor = pieces.next()?.parse().ok()?;
    Some(minor)
}
