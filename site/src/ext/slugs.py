import re, functools, unicodedata

RE_HTML_TAGS = re.compile(r'</?[^>]*>', re.UNICODE)
RE_INVALID_SLUG_CHAR = re.compile(r'[^\w\- ]', re.UNICODE)
RE_WHITESPACE = re.compile(r'\s', re.UNICODE)

def _make_slug(text, sep, **kwargs):
    # If this looks like a changelog slug (... - yyyy-mm-dd), remove the date and return the rest.
    if m := re.match(r'(.*?) - \d{4}-\d{2}-\d{2}', text):
        return m.group(1).strip()
    slug = unicodedata.normalize('NFC', text)
    slug = RE_HTML_TAGS.sub('', slug)
    slug = RE_INVALID_SLUG_CHAR.sub('', slug)
    slug = slug.strip().lower()
    slug = RE_WHITESPACE.sub(sep, slug)
    return slug

def _make_slug_short(text, sep, **kwargs):
    words = _make_slug(text, sep, **kwargs).split(sep)
    return sep.join(words[:5])

def slugify(**kwargs):
    return functools.partial(_make_slug, **kwargs)
