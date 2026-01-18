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
3. **User overrides** (first matching `[[overrides]]` for each setting).
4. **User base config** (`[ui]` section).
5. **Built-in defaults**.

This means CLI arguments and environment variables always override user config.

## Platform-specific overrides

You can customize settings for specific platforms using `[[overrides]]` sections:

```toml title="Platform-specific overrides"
[ui]
# Base settings for all platforms.
show-progress = "auto"
max-progress-running = 8

[[overrides]]
# Windows-specific settings.
platform = "cfg(windows)"
ui.max-progress-running = 4
```

Overrides are evaluated against the *host* platform (where nextest is running). For each setting, the first matching override provides the value. See the [user config reference](reference.md#platform-specific-overrides) for details.

!!! note "User config versus repository config"

    User config primarily affects UI, display settings, and optional features like recording—it does not change test execution behavior. For test execution settings like retries, timeouts, and test groups, use [repository configuration](../configuration/index.md).

## Example configuration

```toml title="User configuration in ~/.config/nextest/config.toml"
# Enable experimental features.
[experimental]
record = true

[ui]
# Always show a progress bar.
show-progress = "bar"

# Show more running tests in the progress output.
max-progress-running = 16

# Disable keyboard input handling.
input-handler = false

# Disable indentation for captured test output.
output-indent = false

[record]
# Enable recording (requires [experimental] record = true).
enabled = true
```

For a complete list of available settings, see the [user config reference](reference.md).

## See also

- [User config reference](reference.md) — complete list of user config settings
- [Record and replay](../features/record-replay.md) — recording test runs for later inspection
- [Repository configuration](../configuration/index.md) — per-repository configuration
