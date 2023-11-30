# Filter expressions

Nextest supports a domain-specific language (DSL) for filtering tests. The DSL is inspired by, and is similar to, [Bazel query](https://bazel.build/docs/query-how-to) and [Mercurial revsets](https://www.mercurial-scm.org/repo/hg/help/revsets).

## Example: Running all tests in a crate and its dependencies

To run all tests in `my-crate` and its dependencies, run:

```
cargo nextest run -E 'deps(my-crate)'
```

The argument passed into the `-E` command-line option is called a _filter expression_. The rest of this page describes the full syntax for the expression DSL.

## The filter expression DSL

A _filter expression_ defines a set of tests. A test will be run if it matches a filter expression.

On the command line, multiple filter expressions can be passed in. A test will be run if it matches any of these expressions. For example, to run tests whose names contain the string `my_test` as well as all tests in package `my-crate`, run:

```
cargo nextest run -E 'test(my_test)' -E 'package(my-crate)'
```

This is equivalent to:

```
cargo nextest run -E 'test(my_test) + package(my-crate)'
```

### Examples

- `package(serde) and test(deserialize)`: every test containing the string `deserialize` in the package `serde`
- `deps(nextest*)`: all tests in packages whose names start with `nextest`, and all of their (possibly transitive) dependencies
- `not (test(/parse[0-9]*/) | test(run))`: every test name not matching the regex `parse[0-9]*` or the substring `run`

> **Note:** If you pass in both a filter expression and a standard, substring-based filter, tests
> must match **both** filter expressions and substring-based filters.
>
> For example, the command:
>
>     cargo nextest run -E 'package(foo)' -- test_bar test_baz
>
> will run all tests that are both in package `foo` and match `test_bar` or `test_baz`.

## DSL reference

This section contains the full set of operators supported by the DSL.

### Basic predicates

- `all()`: include all tests.
- `test(name-matcher)`: include all tests matching `name-matcher`.
- `package(name-matcher)`: include all tests in packages (crates) matching `name-matcher`.
- `deps(name-matcher)`: include all tests in crates matching `name-matcher`, and all of their (possibly transitive) dependencies.
- `rdeps(name-matcher)`: include all tests in crates matching `name-matcher`, and all the crates that (possibly transitively) depend on `name-matcher`.
- `kind(name-matcher)`: include all tests in binary kinds matching `name-matcher`. Binary kinds include:
  - `lib` for unit tests, typically in the `src/` directory
  - `test` for integration tests, typically in the `tests/` directory
  - `bench` for benchmark tests
  - `bin` for tests within `[[bin]]` targets
  - `proc-macro` for tests in the `src/` directory of a procedural macro
- `binary(name-matcher)`: include all tests in binary names matching `name-matcher`.
  - For tests of kind `lib` and `proc-macro`, the binary name is the same as the name of the crate.
  - Otherwise, it's the name of the integration test, benchmark, or binary target.
- `binary_id(name-matcher)`: include all tests in [binary IDs](running.md#binary-ids) matching `name-matcher`.
- `platform(host)` or `platform(target)`: include all tests that are [built for the host or target platform](running.md#filtering-by-build-platform), respectively.
- `none()`: include no tests.

> **Note:** If a filter expression always excludes a particular binary, it will not be run, even to
> get the list of tests within it. This means that a command like:
>
>     cargo nextest list -E 'platform(host)'
>
> will not execute any test binaries built for the target platform. This is generally what you want,
> but if you would like to list tests anyway, include a `test()` predicate. For example, to
> list test binaries for the target platform (using, for example, a [target
> runner](target-runners.md)), but skip running them:
>
>     cargo nextest list -E 'platform(host) + not test(/.*/)' --verbose

### Name matchers

- `=string`: *equality matcher*—match a package or test name that's equal to `string`.
- `~string`: *contains matcher*—match a package or test name containing `string`.
- `/regex/`: *regex matcher*—match a package or test name if any part of it matches the regular expression `regex`. To match the entire string against a regular expression, use `/^regex$/`. The implementation uses the [regex](https://github.com/rust-lang/regex) crate.
- `#glob`: *glob matcher*—match a package or test name if the full name matches the glob expression `glob`. The implementation uses the [globset crate](https://docs.rs/globset).
- `string`: Default matching strategy.
  - For `test()` predicates, this is the *contains matcher*, equivalent to `~string`.
  - For package-related predicates (`package()`, `deps()`, and `rdeps()`), this is the *glob matcher*, equivalent to `#string`.
  - For binary-related predicates (`binary()` and `binary_id()`), this is also the *glob matcher*.
  - For `kind()` and `platform()`, this is the *equality matcher*, equivalent to `=string`.

If you're constructing an expression string programmatically, **always use a prefix** to avoid ambiguity.

#### Escape sequences

The *equality*, *contains*, and *glob* matchers can contain escape sequences, preceded by a
backslash (`\`).

- `\n`: line feed
- `\r`: carriage return
- `\t`: tab
- `\\`: backslash
- `\/`: forward slash
- `\)`: closing parenthesis
- `\,`: comma
- `\u{7FFF}`: 24-bit Unicode character code (up to 6 hex digits)

For the *glob matcher*, to match against a literal glob metacharacter such as `*` or `?`, enclose it in square brackets: `[*]` or `[?]`.

All other escape sequences are invalid.

The _regular expression_ matcher supports the same escape sequences that [the regex crate does](https://docs.rs/regex/latest/regex/#escape-sequences). This includes character classes like `\d`. Additionally, `\/` is interpreted as an escaped `/`.

### Operators

- `set_1 & set_2`, `set_1 and set_2`: the intersection of `set_1` and `set_2`
- `set_1 | set_2`, `set_1 + set_2`, `set_1 or set_2`: the union of `set_1` or `set_2`
- `not set`, `!set`: include everything not included in `set`
- `set_1 - set_2`: equivalent to `set_1 and not set_2`
- `(set)`: include everything in `set`

#### Operator precedence

In order from highest to lowest, or in other words from tightest to loosest binding:

1. `()`
2. `not`, `!`
3. `and`, `&`, `-`
4. `or`, `|`, `+`

Within a precedence group, operators bind from left to right.

##### Examples

- `test(a) & test(b) | test(c)` is equivalent to `(test(a) & test(b)) | test(c)`.
- `test(a) | test(b) & test(c)` is equivalent to `test(a) | (test(b) & test(c))`.
- `test(a) & test(b) - test(c)` is equivalent to `(test(a) & test(b)) - test(c)`.
- `not test(a) | test(b)` is equivalent to `(not test(a)) | test(b)`.

## More information about filter expressions

This section covers additional information that is of interest to nextest's developers and curious readers.

<details>
<summary>Click to expand</summary>

### Motivation

Developer tools often work with some notion of sets, and many of them have grown some kind of domain-specific query language to be able to efficiently specify those sets.

The biggest advantage of a query language is *orthogonality*: rather than every command having to grow a number of options such as `--include` and `--exclude`, developers can learn the query language once and use it everywhere.

### Design decisions

Nextest's filter expressions are meant to be specified at the command line as well as in configuration. This led to the following design decisions:

* **No quotes:** Filter expressions do not have embedded quotes. This lets users use either single (`''`) or double quotes (`""`) to specify filter expressions, without having to worry about escaping them.
* **Minimize nesting of parens:** If an expression language uses parentheses or other brackets heavily (e.g. Rust's [`cfg()` expressions]), getting them wrong can be annoying when trying to write an expression. Text editors typically highlight matching and missing parens, but there's so such immediate feedback on the command line.
* **Infix operators:** Nextest's filter expressions use infix operators, which are more natural to read and write for most people. (As an alternative, Rust's [`cfg()` expressions] use the prefix operators `all()` and `any()`).
* **Operator aliases:** Operators are supported as both words (`and`, `or`, `not`) and symbols (`&`, `|`, `+`, `-`, `!`), letting users write expressions in the style most natural to them. Filter expressions are a small language, so there's no need to be particularly opinionated.

[`cfg()` expressions]: https://doc.rust-lang.org/reference/conditional-compilation.html

</details>
