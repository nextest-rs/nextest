---
icon: material/book-open-variant
sidebar_icon: false
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

## User configuration schema

<!-- md:version 0.9.136 -->

A JSON Schema is available for nextest's user configuration. The schema can be obtained through:

- For the latest released version of nextest, at [`https://nexte.st/schemas/user-config.json`](https://nexte.st/schemas/user-config.json). This URL is updated with new nextest releases.
- For the schema corresponding to a particular version of nextest, by running `cargo nextest self schema user-config`.

The schema can be used:

- To validate a user configuration file.
- With the [Tombi](https://tombi-toml.github.io/tombi) language server for TOML or [RustRover](../integrations/rustrover.md), to provide autocomplete in supported IDEs and editors. (The taplo language server is not supported due to a [crash bug](https://github.com/tamasfe/taplo/pull/779).)

Note that the schema is somewhat stricter than nextest's own config parser: unknown configuration items will fail schema validation, while nextest itself will only print out a warning.

The schema is part of the [JSON Schema Store](https://www.schemastore.org/), so both Tombi and RustRover will automatically download it for you. Because nextest's user configuration (outside of experimental configuration) is [append-only](../stability/index.md), the schema will automatically be updated as new nextest versions are released.

## Experimental features

<!-- md:version 0.9.123 -->

Experimental features are configured under `[experimental]`.

### `experimental.record`

- **Type**: Boolean
- **Description**: Enables the record-replay-rerun feature: stores test run results on disk for replay or selective rerun.
- **Documentation**: [_Record, replay, and rerun_](../features/record-replay-rerun/index.md)
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

For detailed information, see [_Record, replay, and rerun_](../features/record-replay-rerun/index.md).

### `record.enabled`

- **Type**: Boolean
- **Description**: Whether to record test runs. Has no effect unless `[experimental] record = true`.
- **Default**: `false`
- **Example**:
  ```toml
  [record]
  enabled = true
  ```

### `record.max-output-size`

- **Type**: String (size)
- **Description**: Maximum size of a single captured stdout or stderr stream before truncation (e.g. `"10MB"`).
- **Default**: `"10MB"`

### `record.max-records`

- **Type**: Integer
- **Description**: Maximum number of recorded runs to retain before eviction.
- **Default**: `100`

### `record.max-total-size`

- **Type**: String (size)
- **Description**: Maximum combined size of all retained recordings before eviction (e.g. `"1GB"`).
- **Default**: `"1GB"`

### `record.max-age`

- **Type**: String (duration)
- **Description**: Maximum age of a recorded run before eviction (e.g. `"30d"`).
- **Default**: `"30d"`

## UI configuration

UI settings are configured under `[ui]`.

### `ui.show-progress`

<!-- md:version 0.9.118 -->

- **Type**: String
- **Description**: Style of progress display shown during test runs.
- **Valid values**:
  - `"auto"`: picks a display based on terminal capabilities — a progress bar in interactive terminals, a counter otherwise.
  - `"none"`: disables progress display entirely.
  - `"bar"`: shows a progress bar listing the currently running tests.
  - `"counter"`: shows a single-line counter (e.g. `(1/10)`).
  - `"only"`: like `"bar"` in interactive terminals, but additionally hides successful test output (sets `status-level` to `slow` and `final-status-level` to `none`). Falls back to `"auto"` in non-interactive contexts (e.g. piped output, CI).
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
- **Description**: Maximum number of running tests to list in the progress bar. Excess running tests are collapsed into a summary line.
- **Valid values**:
  - Positive integer (e.g. `8`): display up to this many running tests.
  - `0`: hide the list of running tests entirely (the bar still tracks progress).
  - `"infinite"`: display all running tests, no limit.
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
- **Description**: Enables the interactive [keyboard input handler](../reporting.md#live-output) (e.g. `t` to dump test status, `Enter` to print a summary line).
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
- **Description**: Indents captured test output for visual clarity. Applies to output produced by `--failure-output` and `--success-output`.
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
- **Description**: Pager command for output that benefits from scrolling (e.g. `nextest list`, help output). Use `":builtin"` for the builtin pager. See [_Pager support_](pager.md) for details.
- **Valid values**:
  - String: `"less -FRX"` (split on whitespace)
  - Array: `["less", "-FRX"]`
  - Table: `{ command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }`
  - `":builtin"`: use the builtin pager.
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
- **Description**: When to send output through the pager.
- **Valid values**:
  - `"auto"`: pages output from supported commands when stdout is a terminal.
  - `"never"`: disables pagination entirely.
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
- **Description**: How the builtin pager uses the alternate screen and when it exits.
- **Valid values**:
  - `"quit-if-one-page"`: exits immediately if the output fits on one page; otherwise switches to full-screen and clears on exit.
  - `"full-screen-clear-output"`: always uses full-screen mode and clears the screen on exit.
  - `"quit-quickly-or-clear-output"`: waits briefly before entering full-screen mode; clears on exit only if it switched to full-screen.
- **Default**: `"quit-if-one-page"`
- **Example**:
  ```toml
  [ui.streampager]
  interface = "full-screen-clear-output"
  ```

### `ui.streampager.wrapping`

- **Type**: String
- **Description**: How the builtin pager wraps long lines.
- **Valid values**:
  - `"none"`: disables wrapping; long lines extend off-screen and can be scrolled horizontally.
  - `"word"`: wraps at word boundaries.
  - `"anywhere"`: wraps at any character (grapheme) boundary.
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
- **Description**: Target-spec expression selecting which platforms this override applies to (target triple or `cfg()` expression). Matched against the platform nextest was built for. This is distinct from the `platform` field in [per-test overrides](../configuration/per-test-overrides.md#selecting-tests), which is matched against the host or target platform of the tests being run.
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
