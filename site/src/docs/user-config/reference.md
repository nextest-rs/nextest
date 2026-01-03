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
  # Show up to 16 running tests
  max-progress-running = 16
  ```

  ```toml
  [ui]
  # Show all running tests
  max-progress-running = "infinite"
  ```

## Default configuration

The default user configuration is:

```toml
[ui]
show-progress = "auto"
max-progress-running = 8
```
