---
icon: material/account-cog
description: "User configuration: personal preferences for nextest."
---

# User configuration

<!-- md:version 0.9.118 -->

In addition to [repository configuration](../configuration/index.md), nextest supports user-specific configuration. User config stores personal preferences like UI settings.

## Configuration file location

Nextest looks for user configs at the following locations:

| Platform | Primary path | Fallback path |
|----------|--------------|---------------|
| Linux, macOS, and other Unix | `$XDG_CONFIG_HOME/nextest/config.toml` or `~/.config/nextest/config.toml` | — |
| Windows | `%APPDATA%\nextest\config.toml` | `%XDG_CONFIG_HOME%\nextest\config.toml` or `%HOME%\.config\nextest\config.toml` |

On Windows, both the native path (`%APPDATA%`) and the XDG path (`%HOME%\.config`) are checked in order. This allows users who manage dotfiles across platforms to use `~/.config/nextest/config.toml` on Windows as well.

User configuration is not merged; the first matching path is always used.

## Configuration hierarchy

User config settings are resolved with the following precedence (highest priority first):

1. **CLI arguments** (e.g., `--show-progress=bar`).
2. **Environment variables** (e.g., `NEXTEST_SHOW_PROGRESS=bar`).
3. **User config file** (`~/.config/nextest/config.toml`).
4. **Built-in defaults**.

This means CLI arguments and environment variables always override user config.

!!! note "User config versus repository config"

    User config only affects UI and display settings—it does not change test execution behavior. For test execution settings like retries, timeouts, and test groups, use [repository configuration](../configuration/index.md).

## Example configuration

```toml title="User configuration in ~/.config/nextest/config.toml"
[ui]
# Always show a progress bar.
show-progress = "bar"

# Show more running tests in the progress output.
max-progress-running = 16
```

For a complete list of available settings, see the [user config reference](reference.md).

## See also

- [User config reference](reference.md) — complete list of user config settings
- [Repository configuration](../configuration/index.md) — per-repository configuration
