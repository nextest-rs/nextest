# Minimum nextest versions

Starting version 0.9.55, nextest lets you set minimum *required* and *recommended* versions
per-repository. This is similar to the [`rust-version`
field](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field) in
`Cargo.toml`.

* If the current version of nextest is lower than the required version, nextest will produce an error and exit with code 92 ([`REQUIRED_VERSION_NOT_MET`]).
* If the current version of nextest is lower than the recommended version, nextest will produce a warning, but will run as normal.

## Setting minimum versions

To set a minimum required version, add to [`.config/nextest.toml`](configuration.md), at the top of
the file:

```toml
nextest-version = "0.9.55"
# or
nextest-version = { required = "0.9.55" }
```

To set a minimum recommended version, add to `.config/nextest.toml`:

```toml
nextest-version = { recommended = "0.9.55" }
```

Both required and recommended versions can be set simultaneously:

```toml
nextest-version = { required = "0.9.53", recommended = "0.9.55" }
```

> NOTE: Versions of nextest prior to 0.9.55 do not support the `nextest-version` configuration. Depending on how old the version is, nextest may print an "unknown configuration" warning or ignore nextest-version entirely.

## Bypassing the version check

Nextest accepts an `--override-version-check` CLI option that bypasses the version check. If the override is activated, nextest will print a message informing you of that.

<pre><font color="#D3D7CF">% </font><font color="#4E9A06">cargo</font> nextest run --override-version-check
<b>info</b>: overriding version check (required: 0.9.55, current: 0.9.54)
<font color="#4E9A06"><b>    Finished</b></font> test [unoptimized + debuginfo] target(s) in 0.22s
<font color="#4E9A06"><b>    Starting</b></font> <b>191</b> tests across <b>13</b> binaries
...
</pre>

## Showing required and recommended versions

To show and verify the version status, run `cargo nextest show-config version`. This will produce output similar to:

<pre><font color="#D3D7CF">% </font><font color="#4E9A06">cargo</font> nextest show-config version
current nextest version: <b>0.9.54</b>
version requirements:
    - required: <b>0.9.55</b>
evaluation result: <font color="#CC0000"><b>does not meet required version</b></font>
<font color="#CC0000"><b>error</b></font>: update nextest with <b>cargo nextest self update</b>, or bypass check
with --override-version-check
</pre>

This command exits with:

* Exit code 92 ([`REQUIRED_VERSION_NOT_MET`]) if the current version of nextest is lower than the required version.
* Exit code 10 ([`RECOMMENDED_VERSION_NOT_MET`]) if the current version of nextest is lower than the recommended version. This is an advisory exit code that does not necessarily indicate failure.
* Exit code 0 if the version check was satisfied, or if the check was overridden.

## Note for tool developers

If you're building a tool on top of nextest, you can use [tool-specific configuration](configuration.md#tool-specific-configuration) to define minimum required and recommended nextest versions.

**As an exception to the general priority rules** with tool-specific configuration, required and recommended versions across _all_ config files (both repository and tool-specific configurations) are taken into account.

For example, if:

* The repository requires nextest 0.9.54.
* There are two tool config files, and the first one requires nextest 0.9.57.
* The second one requires nextest 0.9.60.

Then, nextest will produce an error unless it is at 0.9.60.

[`REQUIRED_VERSION_NOT_MET`]: https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.REQUIRED_VERSION_NOT_MET
[`RECOMMENDED_VERSION_NOT_MET`]: https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.RECOMMENDED_VERSION_NOT_MET
