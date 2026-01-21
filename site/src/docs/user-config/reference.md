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

## Experimental features

<!-- md:version 0.9.123 -->

Experimental features are configured under `[experimental]`.

### `experimental.record`

- **Type**: Boolean
- **Description**: Enables the record feature
- **Documentation**: [_Record, replay, and rerun_](../features/record-replay-rerun.md)
- **Default**: `false`
- **Environment variable**: `NEXTEST_EXPERIMENTAL_RECORD=1`
- **Example**:
  ```toml
  [experimental]
  record = true
  ```

## Record configuration

<!-- md:version 0.9.123 -->

Record configuration is specified under `[record]`. Recording requires both `[experimental] record = true` and `[record] enabled = true`.

For detailed information, see [_Record, replay, and rerun_](../features/record-replay-rerun.md).

### `record.enabled`

- **Type**: Boolean
- **Description**: Whether to record test runs
- **Default**: `false`
- **Example**:
  ```toml
  [record]
  enabled = true
  ```

### `record.max-output-size`

- **Type**: String (size)
- **Description**: Maximum size per output file before truncation
- **Default**: `"10MB"`

### `record.max-records`

- **Type**: Integer
- **Description**: Maximum number of recorded runs to retain
- **Default**: `100`

### `record.max-total-size`

- **Type**: String (size)
- **Description**: Maximum total size of all recorded runs
- **Default**: `"1GB"`

### `record.max-age`

- **Type**: String (duration)
- **Description**: Maximum age of recorded runs before eviction
- **Default**: `"30d"`

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

### `ui.pager`

<!-- md:version 0.9.120 -->

- **Type**: String, array, or table
- **Description**: Specifies the pager command for output that benefits from scrolling (e.g., `nextest list`, help output). See [_Pager support_](pager.md) for details.
- **Valid values**:
  - String: `"less -FRX"` (split on whitespace)
  - Array: `["less", "-FRX"]`
  - Table: `{ command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }`
  - `":builtin"`: Use the builtin pager
- **Default**: `{ command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }` on Unix, `":builtin"` on Windows (specified via an override)
- **CLI equivalent**: `--no-pager` (to disable paging)
- **Examples**:
  ```toml
  [ui]
  # Use less with custom flags.
  pager = "less -FRX"
  ```

  ```toml
  [ui]
  # Use the builtin pager.
  pager = ":builtin"
  ```

  ```toml
  [ui]
  # Use less with environment variables.
  pager = { command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }
  ```

### `ui.paginate`

<!-- md:version 0.9.120 -->

- **Type**: String
- **Description**: Controls when to paginate output.
- **Valid values**:
  - `"auto"`: Page supported commands if stdout is a terminal
  - `"never"`: Never use a pager
- **Default**: `"auto"`
- **CLI equivalent**: `--no-pager` (equivalent to `paginate = "never"`)
- **Example**:
  ```toml
  [ui]
  # Never use a pager.
  paginate = "never"
  ```

## Builtin pager configuration

<!-- md:version 0.9.120 -->

When `pager = ":builtin"` is set, the builtin pager's behavior can be customized under `[ui.streampager]`. See [_Pager support_](pager.md#builtin-pager-options) for details.

### `ui.streampager.interface`

- **Type**: String
- **Description**: Controls how the builtin pager uses the alternate screen.
- **Valid values**:
  - `"quit-if-one-page"`: Exit immediately if content fits on one page; otherwise use full screen and clear on exit
  - `"full-screen-clear-output"`: Always use full screen mode and clear the screen on exit
  - `"quit-quickly-or-clear-output"`: Wait briefly before entering full screen; clear on exit if entered
- **Default**: `"quit-if-one-page"`
- **Example**:
  ```toml
  [ui.streampager]
  interface = "full-screen-clear-output"
  ```

### `ui.streampager.wrapping`

- **Type**: String
- **Description**: Controls text wrapping in the builtin pager.
- **Valid values**:
  - `"none"`: No wrapping; allow horizontal scrolling
  - `"word"`: Wrap at word boundaries
  - `"anywhere"`: Wrap at any character (grapheme) boundary
- **Default**: `"word"`
- **Example**:
  ```toml
  [ui.streampager]
  wrapping = "none"
  ```

### `ui.streampager.show-ruler`

- **Type**: Boolean
- **Description**: Whether to show a ruler at the bottom of the builtin pager.
- **Valid values**: `true` or `false`
- **Default**: `true`
- **Example**:
  ```toml
  [ui.streampager]
  show-ruler = false
  ```

## Platform-specific overrides

<!-- md:version 0.9.120 -->

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

All `[ui]`, `[ui.streampager]`, and `[record]` settings can be overridden within `[[overrides]]`:

- [`ui.show-progress`](#uishow-progress)
- [`ui.max-progress-running`](#uimax-progress-running)
- [`ui.input-handler`](#uiinput-handler)
- [`ui.output-indent`](#uioutput-indent)
- [`ui.pager`](#uipager)
- [`ui.paginate`](#uipaginate)
- [`ui.streampager.interface`](#uistreampagerinterface)
- [`ui.streampager.wrapping`](#uistreampagerwrapping)
- [`ui.streampager.show-ruler`](#uistreampagershow-ruler)
- [`record.enabled`](#recordenabled)
- [`record.max-output-size`](#recordmax-output-size)
- [`record.max-records`](#recordmax-records)
- [`record.max-total-size`](#recordmax-total-size)
- [`record.max-age`](#recordmax-age)

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
