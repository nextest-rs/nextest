---
icon: material/page-next
description: "Pager support: scrollable output for long listings and help."
---

# Pager support

<!-- md:version 0.9.120 -->

Nextest supports paging output through an external pager (like `less`) or a builtin pager. This is useful for commands that produce long output, such as `cargo nextest list` or `cargo nextest show-config test-groups`.

Nextest's pager support is closely modeled after the [Jujutsu version control system](https://github.com/jj-vcs/jj).

## Paged commands

The following commands support paging:

- [`cargo nextest list`](../listing.md)
- [`cargo nextest replay`](../features/record-replay.md#replaying-test-runs) — replay recorded test runs
- [`cargo nextest store list`](../features/record-replay.md#listing-recorded-runs) — list recorded test runs
- `cargo nextest show-config test-groups` — displays [test group](../configuration/test-groups.md) configuration
- `cargo nextest --help`, `cargo nextest <command> --help` — help output

Paging is automatically disabled when:

- stdout is not an interactive terminal
- machine-readable output is requested (e.g., `--message-format json`)

## Configuration

Pager settings are configured under `[ui]` in the [user config file](index.md#configuration-file-location).

### Choosing a pager

The `pager` setting specifies which pager to use. It supports several formats:

=== "String"

    ```toml title="~/.config/nextest/config.toml"
    [ui]
    pager = "less -FRX"
    ```

    The string is split using Unix shell quoting rules to form the command and arguments.

=== "Array"

    ```toml title="~/.config/nextest/config.toml"
    [ui]
    pager = ["less", "-FRX"]
    ```

    Each element is a separate argument.

=== "Table with environment"

    ```toml title="~/.config/nextest/config.toml"
    [ui]
    pager = { command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }
    ```

    The table format allows setting environment variables for the pager process.

=== "Builtin pager"

    ```toml title="~/.config/nextest/config.toml"
    [ui]
    pager = ":builtin"
    ```

    Use nextest's builtin pager (based on [sapling-streampager]).

[sapling-streampager]: https://crates.io/crates/sapling-streampager

The `PAGER` environment variable is ignored for a better user experience.

### Controlling pagination

The `paginate` setting controls when to use the pager:

```toml title="~/.config/nextest/config.toml"
[ui]
# "auto" (default): page if stdout is a terminal and output benefits from it
# "never": never use a pager
paginate = "auto"
```

### Builtin pager options

When using `pager = ":builtin"`, you can customize the builtin pager's behavior under `[ui.streampager]`:

```toml title="~/.config/nextest/config.toml"
[ui]
pager = ":builtin"

[ui.streampager]
# Interface mode controlling alternate screen behavior.
# "quit-if-one-page" (default): exit immediately if content fits; otherwise use full screen
# "full-screen-clear-output": always use full screen, clear on exit
# "quit-quickly-or-clear-output": wait briefly before entering full screen
interface = "quit-if-one-page"

# Text wrapping mode.
# "none": no wrapping, allow horizontal scrolling
# "word" (default): wrap at word boundaries
# "anywhere": wrap at any character boundary
wrapping = "word"

# Whether to show a ruler at the bottom.
show-ruler = true
```

## Platform defaults

Nextest uses the following default pagers:

| Platform | Default pager |
|----------|---------------|
| Linux, macOS, and other Unix | `less -FRX` with `LESSCHARSET=utf-8` |
| Windows | `:builtin`, since external pagers are unreliable on Windows |

These defaults are configured via [platform-specific overrides](index.md#platform-specific-overrides) in the built-in configuration.

## CLI options

Disable paging for a single invocation with `--no-pager`:

```bash
# List tests without paging
cargo nextest list --no-pager

# Show help for cargo nextest run without paging
cargo nextest run -h --no-pager
```

## Example configurations

### Using a custom pager

```toml title="~/.config/nextest/config.toml"
[ui]
# Use bat as the pager
pager = ["bat", "--style=plain", "--paging=always"]
```

### Disabling paging entirely

```toml title="~/.config/nextest/config.toml"
[ui]
paginate = "never"
```

### Platform-specific pager configuration

To override the pager on Windows, use `[[overrides]]` with `platform = 'cfg(windows)'`:

```toml title="~/.config/nextest/config.toml"
[[overrides]]
platform = "cfg(windows)"
ui.pager = ["bat", "--style=plain", "--paging=always"]
```

## See also

- [User config reference](reference.md) — complete list of user config settings
