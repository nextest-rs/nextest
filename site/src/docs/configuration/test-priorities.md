---
icon: material/priority-high
description: "Reordering tests with nextest so that they run first or last."
---

# Test priorities

<!-- md:version 0.9.91 -->

Nextest allows you to manually reorder tests such that some tests are run first or last. To do so, configure a [per-test override](per-test-overrides.md) with the priority field:

```toml title="Test priorities in <code>.config/nextest.toml</code>"
[[profile.default.overrides]]
# Run these tests with the highest priority.
filter = 'test(high_priority)'
priority = 100

[[profile.default.overrides]]
# Run these tests last.
filter = 'test(final_tests)'
priority = -100
```

Here, `priority` is an integer from -100 to 100, both inclusive. The default priority is 0. Tests with a higher priority run first, with 100 being the highest priority.

Tests with the same priority level are currently run in lexicographic order, and are sorted first by binary name and then by test name. This is not part of the [stability guarantees](../stability/index.md), though any change to this order will be made with care.

## Suggestions for prioritizing tests

Tests should be prioritized with care. Some factors to consider:

* It can be helpful to provide feedback as soon as possible, which suggests running high-signal [smoke tests](https://en.wikipedia.org/wiki/Smoke_testing_(software)) first.
* You may also want to ensure that test runs aren't being held up by slow tests running at the end, which means running the slowest tests first.

Since smoke tests generally tend to be fast, these factors seemingly cut against each other. In general, if you only have a small number of very slow tests, it may help to prioritize both very slow tests and smoke tests before other ones.

!!! warning "Not a way to introduce dependencies between tests"

    It is theoretically possible to use prioritization, along with serial execution via [test groups](test-groups.md), to introduce a dependency between tests. However, this pattern is **not recommended** because the user might choose to only run a subset of tests.

    In general, dependencies between tests should be avoided. To pre-seed some data that multiple tests can use, consider using a [setup script](setup-scripts.md).

## Automatic prioritization

In the future, nextest may gain support for automatic prioritization based on historical test run data. (For example, always run the fastest or slowest tests first.)

Any such automatic prioritization will most likely be opt-in, and will only be used to sort tests within the same priority level. In other words, manual prioritization will always override automatic prioritization.
