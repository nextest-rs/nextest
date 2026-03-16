---
icon: material/package-variant-closed
description: Exporting and sharing recorded test runs as portable recordings.
---

# Portable recordings

<!-- md:version 0.9.125 -->

Recorded runs can be exported as self-contained *portable recordings* for sharing across machines. For example, a recording can be created in CI and downloaded locally to be [replayed](replay.md) or used as the basis for a [rerun](rerun.md).

## Exporting a recording

To export a recording:

```bash
cargo nextest store export latest
```

By default, this creates a file named `nextest-run-<run-id>.zip` in the current directory, where `<run-id>` is the full UUID of the run. The output path can be customized with `--archive-file`:

```bash
cargo nextest store export latest --archive-file my-run.zip
```

## Replaying and rerunning from a portable recording

To replay or rerun from a portable recording, pass the path to the `.zip` file as the `-R` argument:

```bash
# Replay a portable recording.
cargo nextest replay -R my-run.zip

# Rerun failing tests from a portable recording.
cargo nextest run -R my-run.zip
```

<!-- md:version 0.9.127 --> With Unix shells, you can also use [process substitution](https://superuser.com/a/1060002) to download a URL directly:

```bash
# Recommended: =(...) for zsh.
cargo nextest replay -R =(curl https://example.com/archive.zip)

# The <(...) syntax works in both bash and zsh, but is
# slightly less efficient.
cargo nextest replay -R <(curl https://example.com/archive.zip)

# For fish, use psub.
cargo nextest replay -R (curl https://example.com/archive.zip | psub)
```

!!! note "GitHub workflow artifacts"

    If using GitHub's CI, a natural place to upload recordings is as a [GitHub workflow artifact](https://docs.github.com/en/actions/concepts/workflows-and-actions/workflow-artifacts).

    To download these artifacts, the `gh` CLI tool provides [the `gh run download` command](https://cli.github.com/manual/gh_run_download). This command does not currently have a way to write the recording to standard out, so process substitution can't directly be used. Instead, download the archive to disk and use that. For example:

    ```bash
    gh run download 21978978444 -n nextest-run-ubuntu-latest-stable
    cargo nextest replay nextest-run-archive.zip
    ```

!!! warning "Sensitive data in portable recordings"

    Portable recordings contain the full captured output of every test in the run. Test outputs can inadvertently contain sensitive data such as API keys, personal information (PII), or environment variable values. Nextest does not attempt to scrub or redact recordings. You are responsible for ensuring that recordings shared outside your organization do not contain sensitive information.

For more about the portable recording format, see the [design document](../../design/architecture/recording-runs.md#portable-recordings).

## Options and arguments

### `cargo nextest store export`

=== "Summarized output"

    The output of `cargo nextest store export -h`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest store export -h | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest store export -h | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
        ```

=== "Full output"

    The output of `cargo nextest store export --help`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest store export --help | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest store export --help | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
        ```
