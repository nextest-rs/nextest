---
icon: material/filter-check
sidebar_icon: false
description: Reference documentation for nextest's filterset DSL operators and predicates.
---

<!-- include: nextest-filtering/reference.md -->

## More information

This section covers additional information that may be of interest to nextest's developers and curious readers.

### Motivation

Developer tools often work with some notion of sets, and many of them have grown some kind of domain-specific query language to be able to efficiently specify those sets.

The biggest advantage of a query language is _orthogonality_: rather than every command having to grow a number of options such as `--include` and `--exclude`, developers can learn the query language once and use it everywhere.

### Design decisions

Nextest's filtersets are meant to be specified at the command line as well as in configuration. This led to the following design decisions:

- **No quotes:** Filtersets do not have embedded quotes. This lets users use either single (`''`) or double quotes (`""`) to enclose them on the command line, without having to worry about escaping them.
- **Minimize nesting of parens:** If an expression language uses parentheses or other brackets heavily (e.g. Rust's [`cfg()` expressions](https://doc.rust-lang.org/reference/conditional-compilation.html)), getting them wrong can be annoying when trying to write an expression. Text editors typically highlight matching and missing parens, but there's so such immediate feedback on the command line.
- **Infix operators:** Filtersets use infix operators, which are more natural to read and write for most people. (As an alternative, Rust's `cfg()` expressions use the prefix operators `all()` and `any()`).
- **Operator aliases:** Operators are supported as both words (`and`, `or`, `not`) and symbols (`&`, `|`, `+`, `-`, `!`), letting users write expressions in the style most natural to them. Filtersets are a small language, so there's no need to be particularly opinionated.
