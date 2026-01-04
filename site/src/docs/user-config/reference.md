---
icon: material/book-open-variant
description: Reference documentation for all user configuration parameters.
---

# User config reference

This page provides a comprehensive reference of all configuration parameters available in the user config file.

For more information about how user configuration works, see the [user configuration overview](index.md).

## Configuration file location

User configuration is loaded from platform-specific locations:

| Platform | Primary path | Fallback path |
|----------|--------------|---------------|
| Linux, macOS, and other Unix | `$XDG_CONFIG_HOME/nextest/config.toml` or `~/.config/nextest/config.toml` | — |
| Windows | `%APPDATA%\nextest\config.toml` | `%XDG_CONFIG_HOME%\nextest\config.toml` or `%HOME%\.config\nextest\config.toml` |

For more information about configuration hierarchy, see [_Configuration hierarchy_](index.md#configuration-hierarchy).

## UI configuration

UI settings are configured under `[ui]`.

### `ui.show-progress`

<!-- md:version 0.9.118 -->

- **Type**: String
- **Description**: Controls how progress is displayed during test runs
- **Valid values**:
  - `"auto"`: Auto-detect based on terminal capabilities
  - `"none"`: No progress display
  - `"bar"`: Show a progress bar with running tests
  - `"counter"`: Show a simple test counter (e.g., "(1/10)")
  - `"only"`: Show only the progress bar, hide successful test output
- **Default**: `"auto"`
- **CLI equivalent**: `--show-progress`
- **Environment variable**: `NEXTEST_SHOW_PROGRESS`
- **Example**:
  ```toml
  [ui]
  show-progress = "bar"
  ```

### `ui.max-progress-running`

<!-- md:version 0.9.118 -->

- **Type**: Integer or string
- **Description**: Maximum number of running tests to display in the progress bar. When more tests are running than this limit, the progress bar shows the first N tests and a summary (e.g., "... and 24 more tests running").
- **Valid values**:
  - Positive integer (e.g., `8`): Display up to this many running tests
  - `0`: Hide running tests entirely
  - `"infinite"`: Display all running tests
- **Default**: `8`
- **CLI equivalent**: `--max-progress-running`
- **Environment variable**: `NEXTEST_MAX_PROGRESS_RUNNING`
- **Examples**:
  ```toml
  [ui]
  # Show up to 16 running tests.
  max-progress-running = 16
  ```

  ```toml
  [ui]
  # Show all running tests.
  max-progress-running = "infinite"
  ```

### `ui.input-handler`

<!-- md:version 0.9.118 -->

- **Type**: Boolean
- **Description**: Controls whether nextest's [keyboard input handler](../reporting.md#live-output) is enabled. When enabled, nextest accepts keyboard shortcuts during test runs (e.g., `t` to dump test information, `Enter` to produce a summary line).
- **Valid values**: `true` or `false`
- **Default**: `true` (input handler enabled)
- **CLI equivalent**: `--no-input-handler` (to disable)
- **Environment variable**: `NEXTEST_NO_INPUT_HANDLER=1` (to disable)
- **Example**:
  ```toml
  [ui]
  # Disable keyboard input handling.
  input-handler = false
  ```

### `ui.output-indent`

<!-- md:version 0.9.118 -->

- **Type**: Boolean
- **Description**: Controls whether captured test output is indented. By default, test output produced by `--failure-output` and `--success-output` is indented for visual clarity.
- **Valid values**: `true` or `false`
- **Default**: `true`
- **CLI equivalent**: `--no-output-indent` (to disable)
- **Environment variable**: `NEXTEST_NO_OUTPUT_INDENT=1` (to disable)
- **Example**:
  ```toml
  [ui]
  # Disable output indentation.
  output-indent = false
  ```

## Platform-specific overrides

<!-- md:version 0.9.119 -->

You can customize UI settings for specific platforms using `[[overrides]]` sections. This is useful when different platforms need different defaults (e.g., different progress display settings).

### Override structure

```toml title="Platform-specific overrides"
[[overrides]]
platform = "cfg(windows)"
ui.show-progress = "bar"
ui.max-progress-running = 4
```

Each override has a required `platform` filter and optional settings in the `ui` section. Overrides are evaluated in order. For each setting, the first matching override provides the value. If no override matches, the base `[ui]` section value is used.

### Override filter

#### `overrides.platform`

- **Type**: String (target spec expression)
- **Description**: Platform specification for when this override applies. The expression is evaluated against the *host* platform (where nextest is running).
- **Required**: Yes
- **Documentation**: [_Specifying platforms_](../configuration/specifying-platforms.md)
- **Valid formats**:
  - Target triple: `"x86_64-unknown-linux-gnu"`
  - `cfg()` expression: `"cfg(windows)"`, `"cfg(unix)"`, `"cfg(target_os = \"macos\")"`
- **Examples**:
  ```toml
  [[overrides]]
  platform = "cfg(windows)"
  ui.show-progress = "bar"

  [[overrides]]
  platform = "cfg(target_os = \"macos\")"
  ui.max-progress-running = 16
  ```

### Overridable settings

All `[ui]` settings can be overridden within `[[overrides]]`:

- [`ui.show-progress`](#uishow-progress)
- [`ui.max-progress-running`](#uimax-progress-running)
- [`ui.input-handler`](#uiinput-handler)
- [`ui.output-indent`](#uioutput-indent)

Each setting in an override is optional. The first matching override is applied on a per-setting basis.

### Example: platform-specific configuration

```toml title="~/.config/nextest/config.toml"
[ui]
# Base configuration for all platforms.
show-progress = "auto"
max-progress-running = 8

[[overrides]]
# Windows often has narrower terminals.
platform = "cfg(windows)"
ui.max-progress-running = 4

[[overrides]]
# macOS users might prefer a specific setting.
platform = "cfg(target_os = \"macos\")"
ui.show-progress = "bar"
```

### Resolution order

Settings are resolved with the following precedence (highest priority first):

1. **CLI arguments** (e.g., `--show-progress=bar`)
2. **Environment variables** (e.g., `NEXTEST_SHOW_PROGRESS=bar`)
3. **User overrides** (first matching `[[overrides]]` for each setting)
4. **User base config** (`[ui]` section)
5. **Built-in defaults**

## Default configuration

The default user configuration is:

```bash exec="true" result="toml"
cat ../nextest-runner/default-user-config.toml
```
