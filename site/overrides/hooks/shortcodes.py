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

import posixpath
import re

from mkdocs.config.defaults import MkDocsConfig
from mkdocs.structure.files import File, Files
from mkdocs.structure.pages import Page
from re import Match

# -----------------------------------------------------------------------------
# Hooks
# -----------------------------------------------------------------------------

# @todo
def on_page_markdown(
    markdown: str, *, page: Page, config: MkDocsConfig, files: Files
):

    # Replace callback
    def replace(match: Match):
        type, args = match.groups()
        args = args.strip()
        if type == "version":        return _badge_for_version(args, page, files)
        elif type == "flag":         return flag(args, page, files)
        elif type == "option":       return option(args)
        elif type == "setting":      return setting(args)

        # Otherwise, raise an error
        raise RuntimeError(f"Unknown shortcode: {type}")

    # Find and replace all external asset URLs in current page
    return re.sub(
        r"<!-- md:(\w+)(.*?) -->",
        replace, markdown, flags = re.I | re.M
    )

# -----------------------------------------------------------------------------
# Helper functions
# -----------------------------------------------------------------------------

# Create a flag of a specific type
def flag(args: str, page: Page, files: Files):
    type, *_ = args.split(" ", 1)
    if   type == "experimental":  return _badge_for_experimental(page, files)
    elif type == "required":      return _badge_for_required(page, files)
    elif type == "customization": return _badge_for_customization(page, files)
    elif type == "metadata":      return _badge_for_metadata(page, files)
    elif type == "multiple":      return _badge_for_multiple(page, files)
    raise RuntimeError(f"Unknown type: {type}")

# Create a linkable option
def option(type: str):
    _, *_, name = re.split(r"[.:]", type)
    return f"[`{name}`](#+{type}){{ #+{type} }}\n\n"

# Create a linkable setting - @todo append them to the bottom of the page
def setting(type: str):
    _, *_, name = re.split(r"[.*]", type)
    return f"`{name}` {{ #{type} }}\n\n[{type}]: #{type}\n\n"

# -----------------------------------------------------------------------------

# Resolve path of file relative to given page - the posixpath always includes
# one additional level of `..` which we need to remove
def _resolve_path(path: str, page: Page, files: Files):
    path, anchor, *_ = f"{path}#".split("#")
    path = _resolve(files.get_file_from_path(path), page)
    return "#".join([path, anchor]) if anchor else path

# Resolve path of file relative to given page - the posixpath always includes
# one additional level of `..` which we need to remove
def _resolve(file: File, page: Page):
    path = posixpath.relpath(file.src_uri, page.file.src_uri)
    return posixpath.sep.join(path.split(posixpath.sep)[1:])

# -----------------------------------------------------------------------------

# Create badge
def _badge(icon: str, text: str = "", type: str = ""):
    classes = f"mdx-badge mdx-badge--{type}" if type else "mdx-badge"
    return "".join([
        f"<span class=\"{classes}\">",
        *([f"<span class=\"mdx-badge__icon\">{icon}</span>"] if icon else []),
        *([f"<span class=\"mdx-badge__text\">{text}</span>"] if text else []),
        f"</span>",
    ])

# Create badge for version
def _badge_for_version(text: str, page: Page, files: Files):
    spec = text
    path = f"changelog.md#{spec}"

    # Return badge
    icon = "material-tag-outline"
    return _badge(
        icon = f"[:{icon}:]('Minimum nextest version')",
        text = f"[{text}]({_resolve_path(path, page, files)})" if spec else ""
    )


# Create badge for metadata property flag
def _badge_for_metadata(page: Page, files: Files):
    icon = "material-list-box-outline"
    href = _resolve_path("conventions.md#metadata", page, files)
    return _badge(
        icon = f"[:{icon}:]({href} 'Metadata property')"
    )

# Create badge for required value flag
def _badge_for_required(page: Page, files: Files):
    icon = "material-alert"
    href = _resolve_path("conventions.md#required", page, files)
    return _badge(
        icon = f"[:{icon}:]({href} 'Required value')"
    )

# Create badge for customization flag
def _badge_for_customization(page: Page, files: Files):
    icon = "material-brush-variant"
    href = _resolve_path("conventions.md#customization", page, files)
    return _badge(
        icon = f"[:{icon}:]({href} 'Customization')"
    )

# Create badge for multiple instance flag
def _badge_for_multiple(page: Page, files: Files):
    icon = "material-inbox-multiple"
    href = _resolve_path("conventions.md#multiple-instances", page, files)
    return _badge(
        icon = f"[:{icon}:]({href} 'Multiple instances')"
    )

# Create badge for experimental flag
def _badge_for_experimental(page: Page, files: Files):
    icon = "material-flask-outline"
    return _badge(
        icon = f":{icon}:"
    )
