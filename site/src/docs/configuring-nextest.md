---
icon: material/tune
description: "Configuring nextest: repository configuration and user configuration."
---

# Configuring nextest

Nextest has two kinds of configuration:

- [**Repository configuration**](configuration/index.md) is stored in `.config/nextest.toml` at the workspace root. Repository configuration controls test execution behavior: [profiles](configuration/index.md#profiles), [retries](features/retries.md), [timeouts](features/slow-tests.md), [test groups](configuration/test-groups.md), [per-test overrides](configuration/per-test-overrides.md), and more. Repository config is checked into version control and shared across all users of a project.

- [**User configuration**](user-config/index.md) is stored in `~/.config/nextest/config.toml` (or the platform-appropriate equivalent). It controls personal preferences like UI settings, progress display, and [run recording](features/record-replay-rerun/index.md). User config is specific to each user.

For a quick reference of all available settings, see:

- [Repository configuration reference](configuration/reference.md)
- [User configuration reference](user-config/reference.md)
