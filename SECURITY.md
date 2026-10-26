# Security policy

## Supported versions

In general, only the most recently released version of nextest is supported at any time.

For more information about nextest's versioning policy, see [_Stability policy_](https://nexte.st/docs/stability/).

## Security scope

Before reporting vulnerabilities, please understand that part of nextest's job is to run arbitrary Rust code, which is often downloaded from the internet. Most of nextest does not form a trust boundary — for example, [build archives](https://nexte.st/docs/ci-features/archiving/) already run arbitrary code, so a malformed archive doesn't introduce a new attack surface.

## Reporting a vulnerability

Please report vulnerabilities [via GitHub](https://github.com/nextest-rs/nextest/security/advisories/new). The main point of contact is nextest's primary author, Rain Paharia.

We aim to respond to vulnerability reports within 2-3 business days, and to release patches on weekdays shortly afterwards.
