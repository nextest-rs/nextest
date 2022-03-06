# Target runners

If you're cross-compiling Rust code, you may wish to run tests through a wrapper executable or script. For this purpose, nextest supports *target runners*, using the same configuration options used by Cargo:

* The environment variable `CARGO_TARGET_<triple>_RUNNER`, if it matches the target platform, takes highest precedence.
* Otherwise, nextest reads [the `target.<triple>.runner` and `target.<cfg>.runner` settings](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner) from `.cargo/config.toml`.

## Example

If you're on Linux cross-compiling to Windows, you can choose to run tests through [Wine](https://www.winehq.org/).

If you add the following to `.cargo/config.toml:`

```toml
[target.x86_64-pc-windows-msvc]
runner = "wine"
```

Or, in your shell:

```
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine
```

Then, running this command will cause your tests to be run as `wine <test-binary>`:

```
cargo nextest run --target x86_64-pc-windows-msvc
```

> **Note:** If your target runner is a shell script, it might malfunction on macOS due to System Integrity Protection's environment sanitization. This is a system limitation with macOS and not a bug in nextest.
>
> See the discussion in [PR #84] for more.

[PR #84]: https://github.com/nextest-rs/nextest/pull/84
