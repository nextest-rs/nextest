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

from __future__ import annotations

import os
import re

from mkdocs.config.defaults import MkDocsConfig
from mkdocs.structure.files import Files
from mkdocs.structure.pages import Page
from re import Match

def on_page_markdown(
    markdown: str, *, page: Page, config: MkDocsConfig, files: Files
):
    return _expand_includes(markdown, config)

# Inline files referenced via `<!-- include: <path> -->`, resolved relative to the
# repository root (the parent of the MkDocs project directory).
def _expand_includes(markdown: str, config: MkDocsConfig):
    repo_root = os.path.dirname(os.path.dirname(config["config_file_path"]))

    def repl(match: Match):
        rel = match.group(1).strip()
        path = os.path.join(repo_root, rel)
        if not os.path.isfile(path):
            raise RuntimeError(f"include file not found: {rel}")
        with open(path, encoding="utf-8") as f:
            return f.read()

    return re.sub(r"<!-- include:\s*(.*?)\s*-->", repl, markdown)
