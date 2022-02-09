# Listing tests

To build and list all tests in a workspace[^doctest], cd into the workspace and run:

```
cargo nextest list
```

This will produce output that looks something like:

<img src="https://user-images.githubusercontent.com/180618/153310585-48d1aacd-a3a0-4fde-97e8-6b79f2dcb85f.png" width="100%"/>

For each test binary, **bin:** shows the name of the test binary that is run, and **cwd:** shows the name of the working directory the binary will be executed within. Test names are listed below.

[^doctest]: Doctests are currently [not supported](https://github.com/nextest-rs/nextest/issues/16) because of limitations in stable Rust.

`cargo nextest list` takes most of the same options that `cargo nextest run` takes. For a full list of options accepted, see `cargo nextest list --help`.
