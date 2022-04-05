# Filtering expression

* **Introduced in:** cargo-nextest 0.9.13
* **Environment variable**: `NEXTEST_EXPERIMENTAL_EXPR_FILTER=1`
* **Tracking issue**: []

Tests to run can be filtered using filter expressions.

## Filtering expression DSL

A filtering expression define a set of tests, any test in the set will be run.

Basic sets:
- `all()`: include everything
- `test(name-matcher)`: include all tests matching `name-matcher`
- `package(name-matcher)`: include all tests in packages matching `name-matcher`
- `deps(name-matcher)`: include all tests in packages depended on by packages matching `name-matcher` and include all tests in packages matching `name-matcher`
- `rdeps(name-matcher)`: include all tests in packages depending on packages matching `name-matcher` and include all tests in packages matching `name-matcher`
- `none()`: include nothing

Name matcher:
- `unicode_text`: match anything containing `unicode_text`
- `=unicode_text`: match anything equal to `unicode_text`
- `/reg/`: match anything matching the regex `reg`

Unicode string:
- can contains escaped closing parenthesis: `\)`
- can contains unicode sequence: `\u{xxx}` (where `xxx` is an 1 to 6 digits hexadecimal number)

Operations:
- `set_1 & set_2` , `set_1 and set_2`: the intersection of `set_1` and `set_2`
- `set_1 | set_2`, `set_1 or set_2`, `set_1 + set_2`: the union of `set_1` or `set_2`
- `not set`, `!set`: include everything not included in `set`
- `set_1 - set_2`: equivalent to `set_1 and not set_2`
- `(set)`: include everything in `set`

Precedences from lowest to highest:
- `not`, `()`
- `&`, `|`, `-`

Examples:
- `package(=serde) and test(deserialize)`: every tests containing `deserialize` in the package `serde`
- `not (test(parse) | test(run))`: every test not containing `parse` or `run`
- `test(a) & test(b) | test(c)` is equivalent to `( test(a) & test(b) ) | test(c)`
- `test(a) | test(b) & test(c)` is equivalent to `( test(a) | test(b) ) & test(c)`
- `not test(a) | test(b)` is equivalent to `( not test(a) ) | test(b)`

## Usage

Multiple filter expressions can be pass to `cargo nextest`, if a test is include by one of the filtering expressions it will be run.

- `cargo nextest run -E 'package(=crate_a)' -E 'test(parse)'`: will run every tests in the `crate_a` package and every test containing `parse`.
