# Copyright (c) 2016-2024 Martin Donath <martin.donath@squidfunk.com>
# Copyright (c) The nextest Contributors

# Permission is hereby granted, free of charge, to any person obtaining a copy
# of this software and associated documentation files (the "Software"), to
# deal in the Software without restriction, including without limitation the
# rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
# sell copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:

# The above copyright notice and this permission notice shall be included in
# all copies or substantial portions of the Software.

# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NON-INFRINGEMENT. IN NO EVENT SHALL THE
# AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
# FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
# IN THE SOFTWARE.

# Expands `<!-- md:version X -->` and `<!-- md:flag experimental -->` into
# badges.
#
# This is a Markdown extension rather than an MkDocs hook to prepare to run
# under Zensical, which doesn't support hooks.

from __future__ import annotations

import re

from markdown import Markdown
from markdown.extensions import Extension
from markdown.preprocessors import Preprocessor

SHORTCODE_RE = re.compile(r"<!-- md:(\w+)(.*?) -->", re.IGNORECASE)

# The priority sits between pymdownx.snippets (32), so shortcodes in included
# files are expanded, and pymdownx.superfences (31.05 and 25) and html_block
# (20), which would otherwise stash the comments before we see them.
PRIORITY = 31.5


class ShortcodeError(Exception):
    pass


class ShortcodeExtension(Extension):
    def extendMarkdown(self, md: Markdown) -> None:
        md.registerExtension(self)
        md.preprocessors.register(ShortcodePreprocessor(md), "nextest_shortcodes", PRIORITY)


class ShortcodePreprocessor(Preprocessor):
    def run(self, lines: list[str]) -> list[str]:
        text = "\n".join(lines)
        return SHORTCODE_RE.sub(_expand, text).split("\n")


def _expand(match: re.Match[str]) -> str:
    kind, args = match.group(1), match.group(2).strip()
    if kind == "version":
        return _badge_for_version(args)
    elif kind == "flag":
        return _badge_for_flag(args)
    raise ShortcodeError(f"unknown shortcode `{match.group(0)}`: expected `md:version` or `md:flag`")


def _badge_for_version(version: str) -> str:
    if not version:
        raise ShortcodeError("`md:version` requires a version, e.g. `<!-- md:version 0.9.66 -->`")
    # A preprocessor isn't told which page it's rendering, so it can't compute a
    # page-relative link to the changelog. Instead, emit
    # `/changelog.md#<version>`. With `validation.links.absolute_links:
    # relative_to_docs` in mkdocs.yml, MkDocs (and Zensical) treat a leading `/`
    # as the root of `docs_dir` and rewrite the link to be page-relative.
    return _badge(
        icon="[:material-tag-outline:]('Minimum nextest version')",
        text=f"[{version}](/changelog.md#{version})",
    )


def _badge_for_flag(args: str) -> str:
    if args == "experimental":
        return _badge(icon=":material-flask-outline:")
    raise ShortcodeError(f"unknown flag `md:flag {args}`: expected `md:flag experimental`")


def _badge(icon: str, text: str = "") -> str:
    return "".join([
        "<span class=\"mdx-badge\">",
        f"<span class=\"mdx-badge__icon\">{icon}</span>",
        *([f"<span class=\"mdx-badge__text\">{text}</span>"] if text else []),
        "</span>",
    ])


def makeExtension(**kwargs) -> ShortcodeExtension:
    return ShortcodeExtension(**kwargs)
