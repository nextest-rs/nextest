# datatest-stable

[![datatest-stable on crates.io](https://img.shields.io/crates/v/datatest-stable)](https://crates.io/crates/datatest-stable) [![Documentation (latest release)](https://docs.rs/datatest-stable/badge.svg)](https://docs.rs/datatest-stable/) [![Documentation (main)](https://img.shields.io/badge/docs-main-brightgreen)](https://diem.github.io/diem-devtools/rustdoc/datatest_stable/) [![License](https://img.shields.io/badge/license-Apache-green.svg)](../LICENSE-APACHE) [![License](https://img.shields.io/badge/license-MIT-green.svg)](../LICENSE-MIT)

`datatest-stable` is a very simple test harness intended to write data-driven tests, where
individual test cases are specified as data and not as code. Given:
* a test `my_test` that accepts a path as input
* a directory to look for files in
* a pattern to match files on

`datatest_stable` will call the `my_test` function once per matching file in the directory.

This meets some of the needs provided by the `datatest` crate when using a stable rust compiler
without using the `RUSTC_BOOTSTRAP` hack to use nightly features on the stable compiler.

In order to setup data-driven tests for a particular test target you must do the following:

1. Configure the test target by setting the following in the `Cargo.toml`

```toml
[[test]]
name = "<test target name>"
harness = false
```

2. Call the `datatest_stable::harness!(testfn, root, pattern)` macro with the following
parameters:
* `testfn` - The test function to be executed on each matching input. This function must have
  the type `fn(&Path) -> datatest_stable::Result<()>`
* `root` - The path to the root directory where the input files (fixtures) live. This path is
  relative to the root of the crate.
* `pattern` - the regex used to match against and select each file to be tested.

The three parameters can be repeated if you have multiple sets of data-driven tests to be run:
`datatest_stable::harness!(testfn1, root1, pattern1, testfn2, root2, pattern2)`

## Examples

This is an example test. Use it with `harness = false`.

```rust
use std::path::Path;

fn my_test(path: &Path) -> datatest_stable::Result<()> {
    // ... write test here

    Ok(())
}

datatest_stable::harness!(my_test, "path/to/fixtures", r"^.*/*");
```

## See also

* [`datatest`](https://crates.io/crates/datatest): the original inspiration for this crate,
  with a better UI and more features but targeting nightly Rust
* [Data-driven testing](https://en.wikipedia.org/wiki/Data-driven_testing)

## Contributing

See the [CONTRIBUTING](../CONTRIBUTING.md) file for how to help out.

## License

This project is available under the terms of either the [Apache 2.0 license](../LICENSE-APACHE) or the [MIT
license](../LICENSE-MIT).

<!--
README.md is generated from README.tpl by cargo readme. To regenerate:

cargo install cargo-readme
cargo readme > README.md
-->
