# Filter expressions

* **Introduced in:** cargo-nextest 0.9.13 (not released yet)
* **Environment variable**: `NEXTEST_EXPERIMENTAL_FILTER_EXPR=1`
* **Tracking issue**: []

Nextest supports a domain-specific language (DSL) for filtering tests. The DSL is inspired by, and is similar to, [Bazel query](https://bazel.build/docs/query-how-to) and [Mercurial revsets](https://www.mercurial-scm.org/repo/hg/help/revsets).

## Example: Running all tests in a crate and its dependencies

To run all tests in `my-crate` and its dependencies, run:

```
cargo nextest run -E 'deps(=my-crate)'
```

The argument passed into the `-E` command-line option is called a *filter expression*. The rest of this page describes the full syntax for the expression DSL.

## The filter expression DSL

A *filter expression* defines a set of tests. A test will be run if it matches a filter expression.

On the command line, multiple filter expressions can be passed in. A test will be run if it matches any of these expressions. For example, to run tests whose names contain the string `my_test` as well as all tests in package `my-crate`, run:

```
cargo nextest run -E 'test(my_test)' -E 'package(=my-crate)'
```

This is equivalent to:

```
cargo nextest run -E 'test(my_test) + package(=my-crate)'
```

### Examples

- `package(=serde) and test(deserialize)`: every test containing the string `deserialize` in the package `serde`
- `not (test(/parse[0-9]*/) | test(run))`: every test not containing `parse` or `run`

## DSL reference

This section contains the full set of operators supported by the DSL.

### Basic sets

- `all()`: include all tests.
- `test(name-matcher)`: include all tests matching `name-matcher`.
- `package(name-matcher)`: include all tests in packages (crates) matching `name-matcher`.
- `deps(name-matcher)`: include all tests in crates matching `name-matcher`, and all of their (possibly transitive) dependencies.
- `rdeps(name-matcher)`: include all tests in crates matching `name-matcher`, and all the crates that (possibly transitively) depend on `name-matcher`.
- `none()`: include no tests.

### Name matchers

- `string` or `contains:string`: match a package or test name containing `string`
- `=string`: match a package or test name that's equal to `string`
- `/regex/`: match a package or test name if any part of it matches the regular expression `regex`. To match the entire string against a regular expression, use `/^regex$/`. The implementation uses the regex crate.

To match a string beginning with `=` or `/`, or if you're constructing a filter expression in a programmatic context, use the `contains:` prefix.

#### Escape sequences

The *contains* and *equality* name matchers can contain escape sequences, preceded by a backslash (`\`).

* `\n`: line feed
* `\r`: carriage return
* `\t`: tab
* `\\`: backslash
* `\/`: forward slash
* `\)`: closing parenthesis
* `\u{7FFF}`: 24-bit Unicode character code (up to 6 hex digits)

All other escape sequences are invalid.

The *regular expression* matcher supports the same escape sequences that [the regex crate does](https://docs.rs/regex/latest/regex/#escape-sequences). This includes character classes like `\d`. Additionally, `\/` is interpreted as an escaped `/`.

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
