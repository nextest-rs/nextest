---
icon: material/filter-check
---

# Filterset DSL reference

This section contains the full set of operators supported by the filterset DSL.

### Basic predicates

`all()`
: Include all tests.

`none()`
: Include no tests.

`test(name-matcher)`
: Include all tests matching `name-matcher`.

`package(name-matcher)`
: Include all tests in packages (crates) matching `name-matcher`.

`deps(name-matcher)`
: Include all tests in crates matching `name-matcher`, and all of their (possibly transitive) dependencies.

`rdeps(name-matcher)`
: Include all tests in crates matching `name-matcher`, and all the crates that (possibly transitively) depend on `name-matcher`.

`binary_id(name-matcher)`
: Include all tests in [binary IDs](../running.md#binary-ids) matching `name-matcher`. Covers all of `package()`, `kind()` and `binary()`.

`kind(name-matcher)`
: Include all tests in binary kinds matching `name-matcher`. See [_Binary kinds_](#binary-kinds) below.

`binary(name-matcher)`
: Include all tests in binary names matching `name-matcher`. For unit tests, the binary name is the same as the name of the crate. Otherwise, it's the name of the integration test, benchmark, or binary target.

`platform(host)` or `platform(target)`
: Include all tests that are [built for the host or target platform](../running.md#filtering-by-build-platform), respectively.

`default()` <!-- md:version 0.9.75 -->
: The default set of tests to run; see [_Running a subset of tests by default_](../running.md#running-a-subset-of-tests-by-default) for more information.

!!! tip "Binary exclusions"

    If a filterset always excludes a particular binary, it will not be run, even to
    get the list of tests within it. This means that a command like:

        cargo nextest list -E 'platform(host)'

    will not execute any test binaries built for the target platform.

    This is generally what you want, but if you would like to list tests anyway, include a
    `test()` predicate. For example, to list test binaries for the target platform (using,
    for example, a [target runner](../features/target-runners.md)), but skip running them:

        cargo nextest list -E 'platform(host) + not test(/.*/)' --verbose

### Operators

`set1 & set2`, `set1 and set2`
: The intersection of `set1` and `set2`.

`set1 | set2`, `set1 + set2`, `set1 or set2`
: The union of `set1` or `set2`.

`not set`, `!set`
: Include everything not included in `set`

`set1 - set2`
: Everything in `set1` that isn't in `set2`. Equivalent to `set1 and not set2`.

`(set)`
: Include everything in `set`.

#### Operator precedence

In order from highest to lowest, or in other words from tightest to loosest binding:

1. `()`
2. `not`, `!`
3. `and`, `&`, `-`
4. `or`, `|`, `+`

Within a precedence group, operators bind from left to right.

!!! info "Examples"

    - `test(a) & test(b) | test(c)` is equivalent to `(test(a) & test(b)) | test(c)`.
    - `test(a) | test(b) & test(c)` is equivalent to `test(a) | (test(b) & test(c))`.
    - `test(a) & test(b) - test(c)` is equivalent to `(test(a) & test(b)) - test(c)`.
    - `not test(a) | test(b)` is equivalent to `(not test(a)) | test(b)`.

### Binary kinds

Accepted by the `kind()` operator, these binary kinds match the ones defined by Cargo.

`lib`
: Unit tests for regular crates, typically in the `src/` directory under `#[cfg(test)]`.

`test`
: Integration tests, typically in the `tests/` directory.

`bench`
: Benchmark tests. For example, see [_Criterion benchmarks_](../integrations/criterion.md).

`proc-macro`
: Unit tests for proc-macro crates, in the `src/` directory under `#[cfg(test)]`.

`bin`
: Tests within `[[bin]]` targets (uncommon).

`example`
: Tests within examples (uncommon).

### Name matchers

This defines the general syntax for matching against names.

`=string`
: _Equality matcher_—match a package or test name that's equal to `string`.

`~string`
: _Contains matcher_—match a package or test name containing `string`.

`/regex/`
: _Regex matcher_—match a package or test name if any part of it matches the regular expression `regex`. To match the entire string against a regular expression, use `/^regex$/`. The implementation uses the [regex](https://github.com/rust-lang/regex) crate.

`#glob`
: _Glob matcher_—match a package or test name if the full name matches the glob expression `glob`. The implementation uses the [globset crate](https://docs.rs/globset).

`string`
: Default matching strategy for the predicate.

#### Default matchers

For `test()` predicates, the default matching strategy is the _contains matcher_, equivalent to `~string`.

For package-related predicates (`package()`, `deps()`, and `rdeps()`), this is the _glob matcher_, equivalent to `#string`.

For binary-related predicates (`binary()` and `binary_id()`), this is also the _glob matcher_.

For `kind()` and `platform()`, this is the _equality matcher_, equivalent to `=string`.

!!! warning

    If you're constructing an expression string programmatically, **always use a prefix** to avoid ambiguity.

#### Escape sequences

The _equality_, _contains_, and _glob_ matchers can contain escape sequences, preceded by a
backslash (`\`).

<div class="compact" markdown>

`\n`
: line feed

`\r`
: carriage return

`\t`
: tab

`\\`
: backslash

`\/`
: forward slash

`\)`
: closing parenthesis

`\,`
: comma

`\u{7FFF}`
: 24-bit Unicode character code (up to 6 hex digits)

</div>

For the _glob matcher_, to match against a literal glob metacharacter such as `*` or `?`, enclose it in square brackets: `[*]` or `[?]`.

All other escape sequences are invalid.

The _regular expression_ matcher supports the same escape sequences that [the regex crate does](https://docs.rs/regex/latest/regex/#escape-sequences). This includes character classes like `\d`. Additionally, `\/` is interpreted as an escaped `/`.

## More information

This section covers additional information that may be of interest to nextest's developers and curious readers.

### Motivation

Developer tools often work with some notion of sets, and many of them have grown some kind of domain-specific query language to be able to efficiently specify those sets.

The biggest advantage of a query language is _orthogonality_: rather than every command having to grow a number of options such as `--include` and `--exclude`, developers can learn the query language once and use it everywhere.

### Design decisions

Nextest's filtersets are meant to be specified at the command line as well as in configuration. This led to the following design decisions:

- **No quotes:** Filtersets do not have embedded quotes. This lets users use either single (`''`) or double quotes (`""`) to enclose them on the command line, without having to worry about escaping them.
- **Minimize nesting of parens:** If an expression language uses parentheses or other brackets heavily (e.g. Rust's [`cfg()` expressions](https://doc.rust-lang.org/reference/conditional-compilation.html)), getting them wrong can be annoying when trying to write an expression. Text editors typically highlight matching and missing parens, but there's so such immediate feedback on the command line.
- **Infix operators:** Filtersets use infix operators, which are more natural to read and write for most people. (As an alternative, Rust's `cfg()` expressions use the prefix operators `all()` and `any()`).
- **Operator aliases:** Operators are supported as both words (`and`, `or`, `not`) and symbols (`&`, `|`, `+`, `-`, `!`), letting users write expressions in the style most natural to them. Filtersets are a small language, so there's no need to be particularly opinionated.
