---
icon: material/select-group
---

# Target runners

If you're cross-compiling Rust code, you may wish to run tests through a wrapper executable or script. For this purpose, nextest supports _target runners_, using the same configuration options used by Cargo:

- The environment variable `CARGO_TARGET_<triple>_RUNNER`, if it matches the target platform, takes highest precedence.
- Otherwise, nextest reads [the `target.<triple>.runner` and `target.<cfg>.runner` settings](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner) from `.cargo/config.toml`.

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

!!! warning "Shell scripts on macOS"

    If your target runner is a shell script, it might malfunction on macOS due to System Integrity Protection (SIP)'s environment sanitization. Nextest provides the `NEXTEST_LD_*` and `NEXTEST_DYLD_*` environment variables as workarounds. For more, see [_Dynamic linker environment variables_](../installation/macos.md#dynamic-linker-environment-variables).

## Cross-compiling

While cross-compiling code, some tests may need to be run on the host platform. (See [_Filtering by build platform_](../running.md#filtering-by-build-platform) for more.)

For tests that run on the host platform, nextest uses the target runner defined for the host.

For example, if cross-compiling from `x86_64-unknown-linux-gnu` to `x86_64-pc-windows-msvc`, nextest will use:

- `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER` for proc-macro and other host-only tests
- `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER` for other tests.

This behavior is similar to that of [per-test overrides](../configuration/specifying-platforms.md#host-tests).

## Debugging output

Nextest invokes target runners during both the list and run phases. During the list phase, nextest has [stringent rules] for the contents of standard output.

If a target runner produces debugging or any other kind of output, it MUST NOT go to standard output. Instead, you can produce output to standard error, to a file on disk, etc.

For example, this target runner will not work:

```bash
#!/bin/sh
echo "This is some debugging output"
$@
```

Instead, redirect debugging output [to standard error](https://stackoverflow.com/questions/2990414/echo-that-outputs-to-stderr):

```bash
#!/bin/sh
echo "This is some debugging output" >&2
$@
```

[stringent rules]: https://nexte.st/docs/design/custom-test-harnesses/#manually-implementing-a-test-harness
