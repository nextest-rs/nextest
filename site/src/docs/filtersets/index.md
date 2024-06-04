# Filterset DSL

Nextest supports a domain-specific language (DSL) for choosing sets of tests called **filtersets** (formerly **filter expressions**). The DSL is inspired by, and is similar to, [Bazel query](https://bazel.build/docs/query-how-to) and [Mercurial revsets](https://www.mercurial-scm.org/repo/hg/help/revsets).

## Example: Running all tests in a crate and its dependencies

To run all tests in `my-crate` and its dependencies, run:

```
cargo nextest run -E 'deps(my-crate)'
```

## About filtersets

A filterset identifies a set of tests. A test will be included in a filterset if it matches the provided predicates.

On the command line, multiple filtersets can be passed in. A test will be run if it matches any of these expressions. For example, to run tests whose names contain the string `my_test` as well as all tests in package `my-crate`, run:

```
cargo nextest run -E 'test(my_test)' -E 'package(my-crate)'
```

This is equivalent to:

```
cargo nextest run -E 'test(my_test) + package(my-crate)'
```

### Examples of filtersets

`package(serde) and test(deserialize)`
: Matches every test containing the string `deserialize` in the package `serde`

`rdeps(nextest*)`
: Matches all tests in packages whose names start with `nextest` (glob matcher), and all of their reverse dependencies. This includes reverse transitive dependencies.

`not (test(/parse[0-9]*/) | test(run))`
: Matches every test not matching the regex `parse[0-9]*` or the substring `run`.

!!! warning "If both filtersets and substring filters are specified..."

    If you pass in both a filterset and a substring-based filter, tests must match **both** of them. In other words, the union of all filtersets is intersected with the union of substring filters.

    For example, the command:

        cargo nextest run -E 'package(foo)' -- test_bar test_baz

    will run all tests that meet **both** conditions: in package `foo`, and match either `test_bar` or `test_baz`.

## DSL reference

See [_Filterset DSL reference_](reference.md).
