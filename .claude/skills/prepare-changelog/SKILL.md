---
name: prepare-changelog
description: Guidelines for preparing changelog entries for nextest releases following Keep a Changelog format
---

# Changelog Format Guide

This document describes the format and conventions used in `site/src/changelog.md`.

## Overall Structure

The changelog follows the [Keep a Changelog](https://keepachangelog.com/) format with nextest-specific conventions.

### Version Header

```markdown
## [X.Y.Z] - YYYY-MM-DD
```

- Version numbers are in brackets
- Date is in ISO 8601 format (YYYY-MM-DD)
- Each version must have a corresponding link at the bottom of the file.

## Section Organization

Sections should appear in this order (only include sections that are relevant):

1. **Added** - New features
2. **Changed** - Changes to existing functionality
3. **Fixed** - Bug fixes
4. **Deprecated** - Soon-to-be removed features
5. **Removed** - Removed features
6. **Security** - Security-related changes
7. **Known issues** - Known problems with this release
8. **Miscellaneous** - Other notable changes that don't fit elsewhere
9. **Internal improvements** - Internal changes that may interest contributors

### Section Style

- Use `###` for section headers (e.g., `### Added`)
- Each section contains bullet points starting with `-`
- Indent sub-bullets with two spaces

## Content Guidelines

### What to Include

- User-visible changes and new features
- Bug fixes that affect users
- Performance improvements
- Breaking changes (clearly marked)
- MSRV (Minimum Supported Rust Version) changes
- Security updates

### What to Exclude

- Internal dependency updates
- Internal refactoring (unless it has user-visible effects)
- Documentation-only changes to the site
- CI/CD workflow changes
- Dependency updates for minor versions (can be grouped)

### Writing Style

1. **Be concise but descriptive**: Each bullet should clearly explain what changed and why it matters
2. **Use present tense**: "Nextest now supports..." not "Nextest now supported..."
3. **Link to documentation**: When introducing features, link to relevant docs with the full URL path
4. **Include context**: Explain the motivation or benefit when it's not obvious

### Examples

Good:
```markdown
- Nextest can now update itself! Once this version is installed, simply run `cargo nextest self update` to update to the latest version.
```

Good (with note to distributors):
```markdown
- Nextest now sets `NEXTEST_LD_*` and `NEXTEST_DYLD_*` environment variables to work around macOS System Integrity Protection sanitization.
  > Note to distributors: ...
```

Good (with forward-looking context):
```markdown
- A new `threads-required` configuration that can be specified as a per-test override. This can be used to limit concurrency for heavier tests, to avoid overwhelming CPU or running out of memory.
```

## Links and References

### PR and Issue Links

- Use inline links: `([#2618])`
- Define the link at the end of the section or version: `[#2618]: https://github.com/nextest-rs/nextest/pull/2618`
- For pull requests, use the `/pull/` URL
- For issues, use the `/issues/` URL

### External Links

- Use inline markdown links: `[text](URL)`
- Examples: `[GHSA-xxxx](https://github.com/advisories/GHSA-xxxx)`, `[CVE-xxxx](https://nvd.nist.gov/vuln/detail/CVE-xxxx)`

## Contributor Attribution

### First-time Contributors

Always thank first-time contributors using this format (use GitHub username only, not full name):

```markdown
Thanks [username](https://github.com/username) for your first contribution!
```

Place the attribution:
- At the end of the bullet point if it's a single change
- At the end of the section if multiple related changes

Examples:
```markdown
- New feature that does something. Thanks [alice](https://github.com/alice) for your first contribution!
```

```markdown
### Added

- Feature A
- Feature B

Thanks [bob](https://github.com/bob) for your first contribution!
```

### Returning Contributors

For contributors who have contributed before, you can optionally thank them but don't say "first contribution":

```markdown
Thanks [charlie](https://github.com/charlie) for your contribution!
```

Or simply:
```markdown
Thanks [charlie](https://github.com/charlie)!
```

### Multiple Contributors

When multiple people contributed to a feature:
```markdown
Thanks [alice](https://github.com/alice) and [bob](https://github.com/bob) for your contributions!
```

## Special Notations

### Notes to Distributors

Use blockquotes for notes to distributors or package maintainers:

```markdown
> Note to distributors: you can disable self-update by building cargo-nextest with `--no-default-features`.
```

### Upcoming Changes

For warning about future behavior changes:

```markdown
### Upcoming behavior changes

If no tests are run, nextest will start exiting with the advisory code **4** in versions released after 2024-11-18. See [discussion #1646](https://github.com/nextest-rs/nextest/discussions/1646) for more.
```

### Experimental Features

Clearly mark experimental features:

```markdown
- Experimental support for [feature name](link). Please try them out, and provide feedback in the [tracking issue](link)!
```

### Breaking Changes

If a release contains breaking changes, consider adding a note at the top:

```markdown
This is a major release with several new features. It's gone through a period of beta testing, but if you run into issues please [file a bug]!
```

## Formatting Conventions

### Code and Commands

- Use backticks for inline code: `` `cargo nextest run` ``
- Use triple backticks for code blocks with language specification: ` ```toml `, ` ```bash `

### Configuration Examples

When showing configuration:

```markdown
For example, to time out after 120 seconds:

  ```toml
  slow-timeout = { period = "60s", terminate-after = 2 }
  ```
```

Note the indentation for the code block within a bullet point.

### Environment Variables

- Use all caps with backticks: `` `NEXTEST_RETRIES` ``
- Use the format `` `NAME=value` `` when showing how to set them

### Version References

- Cargo versions: "Cargo 1.87"
- Rust versions: "Rust 1.64"
- Nextest versions: "nextest 0.9.100" or "version 0.9.100"

## Dependency Updates

List major dependency updates or security updates separately:

```markdown
- Update rust-openssl for [CVE-2025-24898](https://nvd.nist.gov/vuln/detail/CVE-2025-24898).
```

## Examples of Well-Formed Entries

### Simple Feature Addition

```markdown
### Added

- A new `--hide-progress-bar` option (environment variable `NEXTEST_HIDE_PROGRESS_BAR`) forces the progress bar to be hidden. Thanks [Remo Senekowitsch](https://github.com/remlse) for your first contribution!
```

### Complex Feature with Documentation

```markdown
### Added

- Nextest now supports assigning [test priorities](https://nexte.st/docs/configuration/test-priorities) via configuration.
```

### Bug Fix with Issue Link

```markdown
### Fixed

- Fixed an occasional hang on Linux with [libtest JSON output](https://nexte.st/docs/machine-readable/libtest-json/). For more details, see [#2316].

[#2316]: https://github.com/nextest-rs/nextest/pull/2316
```

### Breaking Change

```markdown
### Changed

- If nextest is unable to parse `--target` (and in particular, a custom target), it now fails rather than printing a warning and assuming the host platform. This is being treated as a bugfix because the previous behavior was incorrect.
```

## Determining What Changed

To generate a changelog entry:

1. Get the commit list: `git log <previous-tag>..main --oneline`
2. Review each commit to determine if it's user-visible
3. Group related commits together (e.g., multiple USDT commits into one feature)
4. Check for first-time contributors: `git log --all --author="Name" --oneline | wc -l`
5. Get PR author GitHub username: `gh pr view <number> --json author --jq '.author.login'`
6. Examine key commits for context: `git show <commit> --stat`

Filter out:
- Documentation site updates (unless they document new features)
- CI configuration changes
- Internal refactoring without user impact
- Most dependency updates (group them together)
