# How to perform nextest releases

## Pre-requisites

Releases depend on:

* [cargo-release](https://github.com/crate-ci/cargo-release), which you can install by running `cargo install cargo-release`. Steps were tested with cargo-release 0.21.4.
* Some way to sign tags. Rain uses and recommends [gitsign](https://github.com/sigstore/gitsign), which uses your GitHub or other online identity to sign tags. Make sure to configure gitsign with:

  ```
  git config --local gpg.x509.program gitsign  # Use gitsign for signing
  git config --local gpg.format x509  # gitsign expects x509 args
  ```

(or `git config --global` to configure this for all repositories)

For a smoother experience, [set `gitsign.connectorID`](https://github.com/sigstore/gitsign#file-config) to the OAuth path you're using.

## Overview

The nextest workspace consists of a set of crates, each independently versioned. Releases are managed in a somewhat distributed way, with part of the process being managed by GitHub Actions runners and part of the process done locally.

## I. Prepare releases

1. First, check out the local branch `main` and fetch/rebase to `origin/main`.
2. Determine which crates need to be released and their version numbers. Most of the time this will just be nextest-runner and cargo-nextest, though if there are changes to other crates they'll need to be released as well.
    * For `nextest-runner`, always update the minor version number (0.27.0 to 0.28.0).
    * For `cargo-nextest`, always update the patch version number (0.9.38 to 0.9.39).
    * For other crates, look at the version.
3. Prepare changelogs. Every crate MUST have a changelog in the [Keep a Changelog](https://keepachangelog.com/en/1.0.0/) format, updated to the latest version. If unreleased changelogs have been maintained, this can be as simple as adding a version header.

(Don't bump the version -- this will be done in the step below.)

## II. Create and push tags

We're going to now use `cargo-release` to create and push tags for each changed crate.

For each changed crate, in topological order (internal dependencies like `nextest-filtering` first, `cargo-nextest` last), run `cargo release -p <crate-name> <version>`. For example, `cargo release -p nextest-runner 0.28.0`. This will perform a dry run.

If everything looks good, run the same command with `--execute`. This will do a few things:

1. Create a commit with a version bump, e.g. `[nextest-runner] version 0.28.0`.
2. Create a signed tag pointing to this commit, e.g. `nextest-runner-0.28.0`.
3. Push this commit and this tag to the remote (default `origin`).

To customize the remote that https://github.com/nextest-rs/nextest is at, use `--push-remote`.

The [GitHub Actions `release.yml` workflow](../.github/workflows/release.yml) will then pick up the tag, and will create a GitHub release corresponding to the tag. Additionally, for `cargo-nextest`, the workflow will also kick off binary builds.

## III. Wait for cargo-nextest builds to complete

If cargo-nextest is being released, wait for builds to complete before publishing the crates in the next step. **This is really important!** Without this, [`cargo binstall`](https://github.com/nextest-rs/nextest/blob/6264dab9b9ca18f1e1e08eb19628cf8534cbc71a/cargo-nextest/Cargo.toml#L57-L67) will temporarily stop working.

You'll know that builds have completed once an "Update release metadata" commit is pushed to main ([example](https://github.com/nextest-rs/nextest/commit/ce9c7fe49b17758b1197b7fa3d2ef6a2c6f9fca2)).

## IV. Publish crates to crates.io

Once cargo-nextest builds are done, from the root of the repository, run:

```
PUBLISH_GRACE_SLEEP=5 cargo release --config release-publish.toml --execute
```

This will publish any crates with changed version numbers to crates.io.
