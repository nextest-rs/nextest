# Pager support design

This document describes the design for adding pager support to nextest, modeled closely on jj's implementation.

## Overview

Nextest will support paging output through an external pager (like `less`) or a builtin pager. This is primarily useful for:

1. **Post-run summary output:** When a test run completes (especially with failures), the summary can be long.
2. **`nextest show-config` commands:** Configuration inspection can produce significant output.
3. **`nextest list` output:** Test lists can be lengthy.

## Reference: jj's pager implementation

jj has a mature pager implementation that we'll use as a reference. Here's how it works:

### Configuration

```toml
[ui]
# Pager command - either an external command or ":builtin"
pager = { command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }
# Or: pager = "less -FRX"
# Or: pager = ":builtin"

# Enable/disable pagination
paginate = "auto"  # or "never"

# Builtin pager options (only when pager = ":builtin")
[ui.streampager]
interface = "quit-if-one-page"  # or "full-screen-clear-output" or "quit-quickly-or-clear-output"
wrapping = "anywhere"           # or "word" or "none"
show-ruler = true
```

**Platform defaults:**
- Unix: `pager = { command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }`
- Windows: `pager = ":builtin"` (external pagers are unreliable on Windows)

### Key design decisions in jj

1. **Does not respect `$PAGER`:** jj intentionally ignores the `$PAGER` environment variable because it often creates a poor out-of-box experience (see [jj#3502](https://github.com/jj-vcs/jj/issues/3502)).

2. **Builtin default on Windows:** External pagers are unreliable on Windows, so `:builtin` is the default there.

3. **Structured command config:** Supports environment variables via `{ command, env }` syntax.

4. **Separate stdout/stderr streams:** The builtin pager handles both, showing stderr in a labeled tab.

5. **On-demand paging:** Commands explicitly call `ui.request_pager()` to enable paging for that command.

### Architecture

```rust
enum UiOutput {
    Terminal { stdout, stderr },     // Direct output
    Paged { child, child_stdin },    // External pager process
    BuiltinPaged { out_wr, err_wr, pager_thread },  // streampager
    Null,                            // Discard output
}
```

**Control flow:**
1. Initialization: `Ui::with_config()` creates a Terminal output and parses pager config.
2. Pager request: Each command that wants paging calls `ui.request_pager()`.
3. Finalization: `ui.finalize_pager()` closes stdin and waits for pager to exit.

### Dependencies

- `sapling-streampager = "0.11.2"` for builtin pager

---

## Nextest pager design

### Scope

For the initial implementation, pager support will apply to:

1. **`nextest list` output** (stdout) — test list display

Future iterations may extend paging to:
- `nextest show-config` output (configuration display)
- Final summary output after a test run (failures and statistics)

For the test runner's live progress, we will *not* use a pager—the progress bar and incremental output must remain interactive.

### Configuration

```toml
# ~/.config/nextest/config.toml

[ui]
# Existing settings...
show-progress = "auto"
max-progress-running = 8
input-handler = true
output-indent = true

# New pager settings
pager = { command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }
# Or: pager = "less -FRX"
# Or: pager = ":builtin"
# Or: pager = "never"  # disable paging

paginate = "auto"  # "auto" | "never"
# auto: page if stdout is a TTY and output exceeds terminal height
# never: never page

# Builtin pager options (when pager = ":builtin")
[ui.streampager]
interface = "quit-if-one-page"
wrapping = "word"
```

### Platform-specific defaults

This is where platform-specific user config overrides become essential:

```toml
# Default for all platforms
[ui]
pager = { command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }

# Windows override
[[overrides]]
platform = "cfg(windows)"
pager = ":builtin"
```

This requires adding `[[overrides]]` support to the user config—see the section below.

### CLI integration

```
--pager <PAGER>          Pager command to use (overrides config)
--no-pager               Disable paging for this invocation
```

When `--no-pager` is specified, it sets `paginate = "never"` equivalent.

### Implementation components

#### 1. Pager types

```rust
/// Pager configuration.
pub enum PagerConfig {
    /// Paging disabled.
    Disabled,
    /// Use the builtin streampager.
    Builtin(StreampagerConfig),
    /// Use an external command.
    External(CommandNameAndArgs),
}

/// Command with optional environment variables.
pub struct CommandNameAndArgs {
    command: Vec<String>,
    env: HashMap<String, String>,
}

/// Streampager configuration.
pub struct StreampagerConfig {
    interface: StreampagerInterface,
    wrapping: StreampagerWrapping,
    show_ruler: bool,
}
```

#### 2. Output wrapper

```rust
/// Wraps output to optionally page it.
pub enum PagedOutput {
    /// Direct output to terminal.
    Terminal {
        stdout: Stdout,
        stderr: Stderr,
    },
    /// Output through external pager.
    ExternalPager {
        child: Child,
        child_stdin: ChildStdin,
    },
    /// Output through builtin pager.
    BuiltinPager {
        out_writer: PipeWriter,
        err_writer: PipeWriter,
        pager_thread: JoinHandle<()>,
    },
}

impl PagedOutput {
    /// Spawn a pager if conditions are met.
    pub fn request_pager(config: &PagerConfig) -> io::Result<Self>;

    /// Finalize the pager (close stdin, wait for exit).
    pub fn finalize(self);
}
```

#### 3. Integration points

**For `nextest list` (initial implementation):**
```rust
// Only page if:
// - stdout is a TTY
// - message_format is human-readable (not json/etc.)
// - paginate != "never"
let should_page = stdout_is_tty
    && message_format.is_human_readable()
    && pager_config.is_enabled();

let mut paged_output = if should_page {
    PagedOutput::request_pager(&pager_config)?
} else {
    PagedOutput::direct()
};

write_test_list(&mut paged_output, &test_list)?;
paged_output.finalize();
```

---

## Platform-specific user config overrides

To provide different defaults on Windows vs Unix, we need to add `[[overrides]]` support to the user config. This is analogous to `[[profile.default.overrides]]` in the repo config.

### Configuration format

```toml
# ~/.config/nextest/config.toml

[ui]
# Base settings (apply to all platforms unless overridden)
show-progress = "auto"
max-progress-running = 8
pager = { command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }

# Platform-specific overrides
[[overrides]]
platform = "cfg(windows)"
ui.pager = ":builtin"
ui.max-progress-running = 4  # Windows terminals may be narrower

[[overrides]]
platform = "cfg(target_os = \"macos\")"
# macOS-specific settings if needed
```

### Override semantics

1. **Platform-only matching:** Unlike repo config overrides which can match on `filter` (test expressions), user config overrides only support `platform` matching. This is because user config applies globally, not per-test.

2. **Host platform evaluation:** Overrides are evaluated against the *host* platform (where nextest is running), not the target platform. This is the natural choice for UI settings.

3. **First-match wins:** Overrides are evaluated in order; the first matching override provides the value. If no override matches, the base `[ui]` section value is used. If that's also unset, the embedded default applies.

4. **Per-setting override:** Each setting is independently overridable. An override that sets `pager` doesn't affect `show-progress`.

### Implementation

#### 1. New types in `user_config/elements/ui.rs`

```rust
/// UI override for platform-specific settings.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UiOverride {
    /// Platform to match (required).
    pub platform: String,

    /// Override settings (all optional).
    #[serde(default)]
    pub show_progress: Option<UiShowProgress>,
    #[serde(default)]
    pub max_progress_running: Option<MaxProgressRunning>,
    #[serde(default)]
    pub input_handler: Option<bool>,
    #[serde(default)]
    pub output_indent: Option<bool>,
    #[serde(default)]
    pub pager: Option<PagerSetting>,
    #[serde(default)]
    pub paginate: Option<PaginateSetting>,
}
```

#### 2. Compiled override

```rust
/// Compiled UI override with parsed platform spec.
pub struct CompiledUiOverride {
    platform_spec: TargetSpec,
    data: UiOverrideData,
}

impl CompiledUiOverride {
    /// Check if this override matches the host platform.
    pub fn matches(&self, host_platform: &Platform) -> bool {
        self.platform_spec
            .eval(host_platform)
            .unwrap_or(false)  // Unknown platforms do not match by default
    }
}
```

#### 3. Resolution logic

```rust
/// Resolved UI configuration after applying overrides.
pub struct ResolvedUiConfig {
    pub show_progress: UiShowProgress,
    pub max_progress_running: MaxProgressRunning,
    pub input_handler: bool,
    pub output_indent: bool,
    pub pager: PagerConfig,
    pub paginate: PaginateSetting,
}

impl ResolvedUiConfig {
    pub fn resolve(
        user_config: Option<&UserConfig>,
        default_config: &DefaultUserConfig,
        host_platform: &Platform,
    ) -> Self {
        // Start with defaults
        let mut result = Self::from_defaults(default_config);

        // Apply base user config
        if let Some(user) = user_config {
            result.apply_base(&user.ui);
        }

        // Apply matching overrides in order
        if let Some(user) = user_config {
            for override_ in &user.overrides_ui {
                if override_.matches(host_platform) {
                    result.apply_override(override_);
                }
            }
        }

        result
    }
}
```

#### 4. Updated `UserConfig` structure

```rust
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UserConfig {
    /// UI configuration.
    #[serde(default)]
    pub ui: UiConfig,

    /// UI overrides for platform-specific settings.
    #[serde(default, rename = "overrides")]
    overrides: UserConfigOverrides,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UserConfigOverrides {
    #[serde(default)]
    pub ui: Vec<UiOverride>,
}
```

#### 5. CLI integration changes

The CLI currently receives `Option<&UiConfig>`. We need to change this to receive `ResolvedUiConfig` instead:

```rust
// Before
let user_config = UserConfig::from_default_location().map_err(Box::new)?;
reporter_opts.to_builder(
    // ...
    user_config.as_ref().map(|c| &c.ui),
    &default_user_config.ui,
);

// After
let user_config = UserConfig::from_default_location().map_err(Box::new)?;
// Note: HostPlatform::detect() requires a PlatformLibdir, but for user config
// we only need the Platform for matching. We can use a simpler detection path
// or reuse the host platform from BuildPlatforms if already computed.
let host_platform = HostPlatform::detect(PlatformLibdir::Unavailable(...))?;
let resolved_ui = ResolvedUiConfig::resolve(
    user_config.as_ref(),
    &default_user_config,
    &host_platform.platform,
);
reporter_opts.to_builder(
    // ...
    &resolved_ui,
);
```

### Default user config changes

The embedded default-user-config.toml would be updated:

```toml
# This is the default user config used by nextest.

[ui]
show-progress = "auto"
max-progress-running = 8
input-handler = true
output-indent = true
pager = { command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }
paginate = "auto"

# Windows uses builtin pager by default
[[overrides]]
platform = "cfg(windows)"
ui.pager = ":builtin"
```

---

## Implementation plan

### Phase 1: Platform-specific user config overrides

1. Add `UiOverride` and `CompiledUiOverride` types.
2. Add `overrides` parsing to `UserConfig`.
3. Add `UiConfig` with resolution logic.
4. Update CLI to use `UiConfig` instead of raw `UiConfig` (which turns into `DeserializedUiConfig`).
5. Add tests for override matching and resolution.

### Phase 2: Pager infrastructure

1. Add `PagerConfig` and `CommandNameAndArgs` types.
2. Add `pager` and `paginate` fields to `UiConfig` and `UiOverride`.
3. Add pager parsing (string, array, or table formats).
4. Add `PagedOutput` wrapper with external pager support.
5. Add tests for pager spawning and finalization.

### Phase 3: Builtin pager

1. Add `sapling-streampager` dependency.
2. Add `StreampagerConfig` parsing.
3. Implement builtin pager output variant.
4. Add streampager-specific options (`[ui.streampager]`).
5. Test on Unix and Windows.

### Phase 4: Integration

1. Add `--pager` and `--no-pager` CLI options.
2. Integrate pager with `nextest list` output (stdout only).
3. Disable paging when `--message-format` is set to a structured format.
4. Documentation updates.

**Future work:**
- Integrate pager with `nextest show-config` output.
- Integrate pager with post-run summary output (stderr).

---

## Design decisions

1. **Pager applies to stdout only (initially).** The initial implementation will only page `nextest list` output, which goes to stdout. Stderr-based output (progress bar, live output, final summary) will be considered in a future iteration. This keeps the initial scope manageable.

2. **No paging for structured output.** When `--message-format json` or other machine-readable formats are used, paging is disabled. Machine-readable output is typically piped to other tools.

3. **Graceful fallback on pager failure.** If the pager fails to spawn (e.g., command not found), print a warning and fall back to direct output. This matches jj's behavior.

4. **Graceful signal handling.** When the user quits the pager (e.g., pressing `q` in `less`), the pager exits with SIGPIPE or similar. This is normal behavior and should not be treated as an error.

---

## Phase 1 implementation reference

**Files to modify for platform-specific user config overrides:**

| File | Changes |
|------|---------|
| `nextest-runner/src/user_config/mod.rs` | Export new types |
| `nextest-runner/src/user_config/imp.rs` | Add `overrides` field to `UserConfig`, compilation logic |
| `nextest-runner/src/user_config/elements/ui.rs` | Add `UiOverride` type, `ResolvedUiConfig` |
| `nextest-runner/default-user-config.toml` | Add Windows override for pager |
| `cargo-nextest/src/dispatch/cli.rs` | Change `ReporterOpts::to_builder` to use `ResolvedUiConfig` |
| `cargo-nextest/src/dispatch/execution.rs` | Resolve UI config with host platform before passing to builder |

**Key existing code patterns to follow:**

- `MaybeTargetSpec` in `nextest-runner/src/config/overrides/imp.rs:886-910` — platform spec parsing and evaluation
- `PlatformStrings` deserializer in same file `:995-1043` — flexible string/table format
- `target_spec::TargetSpec::new()` and `.eval(&platform)` for matching

**Dependencies already available:**
- `target-spec` — already used in nextest-runner
- `HostPlatform::detect()` in `nextest-runner/src/platform.rs:206` — queries `rustc -vV` with fallback to `Platform::build_target()`
- Access `host_platform.platform` to get the `Platform` for `TargetSpec::eval()`

---

## References

- jj pager implementation: `cli/src/ui.rs`
- jj config documentation: `docs/config.md`
- sapling-streampager: https://crates.io/crates/sapling-streampager
- target-spec crate: https://crates.io/crates/target-spec
