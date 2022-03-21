## Pull Requests

If you have a new feature in mind, please discuss the feature in an issue to ensure that your
contributions will be accepted.

1. Fork the repo and create your branch from `main`.
2. If you've added code that should be tested, add tests.
3. If you've changed APIs, update the documentation.
4. Ensure the test suite passes with `cargo nextest run --all-features`.
   
    > NOTE: Nextest's own tests do not work with `cargo test`. You must [install nextest](https://nexte.st/book/pre-built-binaries) to run its own test suite.

6. Run `cargo xfmt` to automatically format your changes (CI will let you know if you missed this).

## Logically Separate Commits

Commits should be
[atomic](https://en.wikipedia.org/wiki/Atomic_commit#Atomic_commit_convention)
and broken down into logically separate changes. Diffs should also be made easy
for reviewers to read and review so formatting fixes or code moves should not
be included in commits with actual code changes.

## Bisect-able History

It is important that the project history is bisect-able so that when
regressions are identified we can easily use `git bisect` to be able to
pin-point the exact commit which introduced the regression. This requires that
every commit is able to be built and passes all lints and tests. So if your
pull request includes multiple commits be sure that each and every commit is
able to be built and passes all checks performed by CI.

## License

By contributing to `cargo-nextest`, you agree that your contributions will be dual-licensed under
the terms of the [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE) files in the
root directory of this source tree.
