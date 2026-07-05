---
icon: material/book-open-variant
sidebar_icon: false
description: Reference documentation for all user configuration parameters.
---

# User config reference

This page provides the full user configuration reference for nextest.

<!-- md:version 0.9.140 --> This reference is also available locally by running `cargo nextest help user-config`.

For more information about how user configuration works, see the [user configuration overview](index.md).

## Configuration file location

User configuration is loaded from platform-specific locations:

| Platform | Primary path | Fallback path |
|----------|--------------|---------------|
| Linux, macOS, and other Unix | `$XDG_CONFIG_HOME/nextest/config.toml` or `~/.config/nextest/config.toml` | — |
| Windows | `%APPDATA%\nextest\config.toml` | `%XDG_CONFIG_HOME%\nextest\config.toml` or `%HOME%\.config\nextest\config.toml` |

For more information about configuration hierarchy, see [_Configuration hierarchy_](index.md#configuration-hierarchy).

<!-- include: nextest-runner/user-config-reference.md -->

## Default configuration

The default user configuration is:

```bash exec="true" result="toml"
cat ../nextest-runner/default-user-config.toml
```
