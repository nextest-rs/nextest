---
icon: material/hand-heart
description: Guidelines for contributing to cargo-nextest.
---

# Contributing

## Pull requests

If you have a new feature in mind, please discuss the feature in an issue to ensure that your
contributions will be accepted.

1. Fork the repo and create your branch from `main`.
2. If you've added code that should be tested, add tests.
3. If you've changed APIs, update the documentation.
4. Ensure the test suite passes with `cargo nextest run --all-features`.

  !!! note

      Nextest's own tests do not work with `cargo test`. You must [install
      nextest](installation/pre-built-binaries.md) to run its own test suite.

5. Run `cargo xfmt` to automatically format your changes (CI will let you know if you missed this).

Nextest aims to provide a high-quality, polished user experience. If you're adding a new
feature, please pay attention to:

- [Coloring support](https://rust-cli-recommendations.sunshowers.io/colors.html).
- [Configuration](https://rust-cli-recommendations.sunshowers.io/configuration.html), including hierarchical configuration.
- Error handling. In particular, errors caused by components outside of nextest itself _should_ be part of [`ExpectedError`](https://github.com/nextest-rs/nextest/blob/main/cargo-nextest/src/errors.rs) and use a [well-defined exit code](https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html).

## Code conventions

For code conventions followed by the nextest project, see [AGENTS.md](https://github.com/nextest-rs/nextest/blob/main/AGENTS.md). This file targets LLMs but is appropriate for humans as well.

## Logically separate commits

Commits should be
[atomic](https://en.wikipedia.org/wiki/Atomic_commit#Atomic_commit_convention)
and broken down into logically separate changes. Diffs should also be made easy
for reviewers to read and review so formatting fixes or code moves should not
be included in commits with actual code changes.

## Bisectable history

It is important that the project history is bisectable, so that when
regressions are identified we can easily use `git bisect` to be able to
pinpoint the exact commit which introduced the regression. This requires that
every commit is able to be built, and passes all lints and tests. So, if your
pull request includes multiple commits, be sure that every commit is
able to be built and passes all checks performed by CI.

## LLM and AI policy

### LLMs for code

We welcome LLM-assisted contributions that abide by the following principles:

* **Aim for excellence.** For the nextest project, LLMs should be used not as a speed multiplier but a quality multiplier. Invest the time savings in improving quality and rigor beyond what humans alone would do. Write tests that cover more edge cases. Refactor code to make it easier to understand. Tackle the TODOs. Do all the tedious things. Aim for your code to have zero bugs.
* **Spend time reviewing LLM output.** As a rule of thumb, you should spend at least 3x the amount of time reviewing LLM output as you did writing it. Think about design decisions and alternatives. (Tip: LLMs are very good reviewers — consider using a different model or a fresh context to review code, systematically working through the issues identified by it.)
* **Your code is your responsibility.** Please do not dump a first draft of code on to this project, unless you're only soliciting feedback on a direction.

If your LLM-assisted PR shows signs of not being written with thoughtfulness and care, such as missing cases that human review would have easily caught, nextest's maintainers may decline the PR outright.

### LLMs for issues and bug reports

Nextest does not accept issues and bug reports that are primarily authored by LLMs (AI slop prose). We would like to communicate with a human on the other side, not an agent.

* Using LLMs as editors and proofreaders is acceptable. We are not ideologically opposed to AI; rather, we think AI slop prose is overly verbose, difficult to read, and wasteful of our time.
* If you don't feel comfortable communicating in English, please write in your native language — the nextest maintainers are happy to use machine translation tools on our end.

As an exception, _extra information_ that can be relevant for debugging an issue (e.g., a diagnosis performed by an LLM) can be posted under a collapsible section using the following template:

```html
<details>

<summary>LLM-generated information</summary>

Details here...

</details>
```

The goal of this is to center humans and keep AI slop prose out of the way as much as possible.

## License

By contributing to `cargo-nextest`, you agree that your contributions will be dual-licensed under
the terms of the [`LICENSE-MIT`](https://github.com/nextest-rs/nextest/blob/main/LICENSE-MIT) and [`LICENSE-APACHE`](https://github.com/nextest-rs/nextest/blob/main/LICENSE-APACHE) files in the
root directory of this source tree.
