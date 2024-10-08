---
icon: material/laptop
---

# Specifying platforms for per-test settings

[Per-test overrides](per-test-overrides.md) support filtering by platform. Either a Rust [target triple](https://doc.rust-lang.org/beta/rustc/platform-support.html#platform-support) or [`cfg()` expression](https://doc.rust-lang.org/reference/conditional-compilation.html) may be specified.

For example, with the following configuration:

```toml title="Platform overrides in <code>.config/nextest.toml</code>"
[[profile.default.overrides]]
platform = 'cfg(target_os = "linux")'
retries = 3
```

Test runs on Linux will have 3 retries.

## Cross-compiling

While cross-compiling code, nextest's per-test overrides support filtering by either _host_ or _target_ platforms.

If `platform` is set to a string, then nextest will consider it to be the _target_ filter. For example, if the following is specified:

```toml title
[[profile.default.overrides]]
platform = 'aarch64-apple-darwin'
slow-timeout = "120s"
```

Then test runs performed either natively on `aarch64-apple-darwin`, or while cross-compiling from some other operating system to `aarch64-apple-darwin`, will be marked slow after 120 seconds.

### Filtering by both host and target

<!-- md:version 0.9.58 -->

In addition to a plain string, `platform` can also be set to a map with `host` and `target` keys. While determining whether a particular override applies, nextest will apply both host and target filters (AND operation).

For example:

```toml title="Cross-compile overrides"
[[profile.default.overrides]]
platform = { host = 'cfg(target_os = "macos")' }
retries = 1

[[profile.default.overrides]]
platform = { host = 'x86_64-unknown-linux-gnu', target = 'cfg(windows)' }
threads-required = 2
```

With this configuration:

- On macOS hosts (regardless of the target platform), tests will be retried once.
- On x86_64 Linux hosts, while cross-compiling to Windows, tests will be marked as requiring two threads each.

!!! tip

    Specifying `platform` as a string is equivalent to specifying it as a map with the `target` key.

### Host tests

While cross-compiling code, some tests may need to be run on the host platform. (See the note about [Filtering by build platform](../running.md#filtering-by-build-platform) for more.)

For tests that run on the host platform, to figure out if an override applies nextest will compute the result of the _target_ filter against the _host_ platform. (If the `host` key is specified, it will be considered as well based on the AND semantics listed above.)

This behavior is similar to that of [target runners](../features/target-runners.md#cross-compiling).
