# Listing tests

To build and list all tests in a workspace[^doctest], cd into the workspace and run:

```
cargo nextest list
```

`cargo nextest list` takes most of the same options that `cargo nextest run` takes. For a full list of options accepted, see `cargo nextest list --help`.

![Output of cargo nextest list](../static/nextest-list.png)

[^doctest]: Doctests are currently [not supported](https://github.com/nextest-rs/nextest/issues/16) because of limitations in stable Rust.

## Options and arguments

```
{{#include ../../help-text/list-help.txt}}
```
