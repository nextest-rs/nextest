// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Renders MkDocs help-topic documents (help topics) to the terminal.

use crate::{config::core::NextestConfig, helpers::RESET_COLOR, user_config::UserConfig};
use indoc::formatdoc;
use nextest_filtering::FILTERSET_REFERENCE_MD;
use owo_colors::Style;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use smallvec::SmallVec;
use std::borrow::Cow;
use swrite::{SWrite, swrite};
use synoptic::{TokOpt, from_extension};
use tracing::warn;
use unicode_width::UnicodeWidthStr;

const SITE_BASE: &str = "https://nexte.st";

/// A help topic document.
pub struct HelpDoc {
    /// The raw markdown content of the document.
    ///
    /// This is transformed by the renderer.
    pub markdown: Cow<'static, str>,
    /// The doc's directory within the site, e.g. `["docs", "filtersets"]`.
    ///
    /// This is used to resolve relative links within the document.
    pub site_dir: &'static [&'static str],
}

impl HelpDoc {
    /// The filterset reference document.
    pub fn filterset() -> Self {
        let markdown = formatdoc! {"
                # Filterset DSL reference

                This topic contains the full set of operators supported by the filterset DSL.

                This reference is also available [on the nextest site](https://nexte.st/docs/filtersets/reference/).

                {FILTERSET_REFERENCE_MD}
            "};
        Self {
            markdown: Cow::Owned(markdown),
            site_dir: &["docs", "filtersets"],
        }
    }

    /// The repository configuration reference document.
    pub fn repo_config() -> Self {
        Self {
            markdown: Cow::Owned(repo_config_markdown()),
            site_dir: &["docs", "configuration"],
        }
    }

    /// The user configuration reference document.
    pub fn user_config() -> Self {
        Self {
            markdown: Cow::Owned(user_config_markdown()),
            site_dir: &["docs", "user-config"],
        }
    }
}

fn repo_config_markdown() -> String {
    formatdoc! {"
            # Configuration reference

            This topic contains the full repository configuration reference for nextest.

            This reference is also available [on the nextest site](https://nexte.st/docs/configuration/reference/).

            {reference}

            ## Default configuration

            The default configuration shipped with cargo-nextest is:

            ```toml
            {default_config}
            ```
        ",
        reference = NextestConfig::REFERENCE_MD.trim_end(),
        default_config = NextestConfig::DEFAULT_CONFIG.trim_end(),
    }
}

fn user_config_markdown() -> String {
    formatdoc! {r#"
            # User config reference

            This topic contains the full user configuration reference for nextest.

            This reference is also available [on the nextest site](https://nexte.st/docs/user-config/reference/).

            For more information about how user configuration works, see the [user configuration overview](index.md).

            ## Configuration file location

            User configuration is loaded from one of the following platform-specific locations:

            1. On Linux, macOS, and other Unix platforms: `$XDG_CONFIG_HOME/nextest/config.toml`, or `~/.config/nextest/config.toml` if `$XDG_CONFIG_HOME` is unset or empty.
            2. On Windows: `%APPDATA%\nextest\config.toml`, falling back to `%XDG_CONFIG_HOME%\nextest\config.toml`, then `%HOME%\.config\nextest\config.toml`.

            For more information about configuration hierarchy, see [_Configuration hierarchy_](index.md#configuration-hierarchy).

            {reference}

            ## Default configuration

            The default user configuration is:

            ```toml
            {default_config}
            ```
        "#,
        reference = UserConfig::REFERENCE_MD.trim_end(),
        default_config = UserConfig::DEFAULT_CONFIG.trim_end(),
    }
}

/// Rendering options for help topic output.
pub struct RenderOptions {
    /// Whether to emit ANSI color and style codes.
    pub color: bool,
    /// Whether to emit OSC-8 hyperlinks for links.
    pub hyperlinks: bool,
    /// The maximum width, in columns, to wrap output to.
    pub width: usize,
}

/// Renders a help topic document to terminal-ready text.
pub fn render(doc: &HelpDoc, opts: RenderOptions) -> String {
    let preprocessed = preprocess(&doc.markdown);
    let rendered = render_markdown(&preprocessed, doc.site_dir, opts);
    if !rendered.dropped.is_empty() {
        warn!(
            "help renderer dropped unsupported markdown events: {:?}",
            rendered.dropped
        );
    }
    rendered.output
}

/// Preprocesses MkDocs markdown into a plain CommonMark subset.
fn preprocess(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // Drop raw <div> wrappers -- no real way to support them in terminal
        // output.
        if trimmed.starts_with("<div") || trimmed == "</div>" {
            i += 1;
            continue;
        }

        // Convert admonitions into block quotes.
        if let Some(header) = admonition_header(trimmed) {
            let indent = line.len() - trimmed.len();
            let body_indent = indent + 4;
            i += 1;

            let mut body_lines: Vec<String> = Vec::new();
            while i < lines.len() {
                let l = lines[i];
                if l.trim().is_empty() {
                    body_lines.push(String::new());
                    i += 1;
                } else if leading_spaces(l) >= body_indent {
                    body_lines.push(l[l.ceil_char_boundary(body_indent)..].to_string());
                    i += 1;
                } else {
                    break;
                }
            }
            while body_lines.last().is_some_and(|s| s.is_empty()) {
                body_lines.pop();
            }

            out.push(header);
            out.push(">".to_string());
            for b in body_lines {
                if b.is_empty() {
                    out.push(">".to_string());
                } else {
                    out.push(format!("> {b}"));
                }
            }
            out.push(String::new());
            continue;
        }

        out.push(line.to_string());
        i += 1;
    }

    let joined = out.join("\n");
    apply_shortcodes(&joined)
}

fn leading_spaces(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Parses a `!!! kind "Title"` admonition header into a block-quote header
/// line.
fn admonition_header(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix("!!! ")?;
    let (kind, title) = match rest.split_once(' ') {
        Some((kind, title)) => (kind, Some(title.trim())),
        None => (rest, None),
    };
    let label = capitalize(kind);
    let title = title.and_then(|t| t.strip_prefix('"').and_then(|t| t.strip_suffix('"')));
    Some(match title {
        Some(title) => format!("> **{label}: {title}**"),
        None => format!("> **{label}**"),
    })
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Replaces `<!-- md:version X -->` with inline text, and strips other HTML
/// comments.
fn apply_shortcodes(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "<!--".len()..];
        let Some(end) = after.find("-->") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = after[..end].trim();
        let version = inner
            .strip_prefix("md:version")
            .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
        if let Some(version) = version {
            let version = version.trim();
            if !version.is_empty() {
                swrite!(out, "*(since {version})*");
            }
        }
        rest = &after[end + "-->".len()..];
    }
    out.push_str(rest);
    out
}

/// Rewrites a relative URL to an absolute `nexte.st` URL.
///
/// Returns `None` for same-page anchors, which are not rewritten.
fn rewrite_url(url: &str, site_dir: &[&str]) -> Option<String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(url.to_string());
    }

    let (path, anchor) = match url.split_once('#') {
        Some((path, anchor)) => (path, Some(anchor)),
        None => (url, None),
    };

    // The target is a same-page anchor (e.g. [foo](#foo)). The content it
    // points to is already on screen, so drop the link.
    if path.is_empty() {
        return None;
    }

    let mut segments: Vec<String> = site_dir.iter().map(|s| s.to_string()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                // mkdocs would warn about this as well.
                assert!(
                    !segments.is_empty(),
                    "relative link in help doc escapes the site root: {url:?}"
                );
                segments.pop();
            }
            // With our configuration, mkdocs maps `index.md` to its containing
            // directory's URL, so this should be ignored.
            "index.md" => {}
            other => segments.push(other.trim_end_matches(".md").to_string()),
        }
    }
    let resolved = format!("{SITE_BASE}/{}/", segments.join("/"));

    Some(match anchor {
        Some(anchor) => format!("{resolved}#{anchor}"),
        None => resolved,
    })
}

const DEFINITION_INDENT: &str = "    ";
const QUOTE_PREFIX: &str = "│ ";
const CODE_INDENT: &str = "    ";

/// A stack of desired or currently-applied styles.
///
/// A capacity of 4 should cover all body text and headings in practice, and
/// most of them in theory.
type StyleStack = SmallVec<[Style; 4]>;

/// The target of a hyperlink for a run of text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum LinkTarget {
    /// The text is not linked.
    #[default]
    Unlinked,
    /// The text is linked to the given URL.
    Linked(String),
}

impl LinkTarget {
    fn is_linked(&self) -> bool {
        match self {
            LinkTarget::Unlinked => false,
            LinkTarget::Linked(_) => true,
        }
    }
}

/// The presentation attributes of a run of text.
#[derive(Clone, Debug, Default)]
struct RunAttrs {
    styles: StyleStack,
    link: LinkTarget,
}

impl RunAttrs {
    /// Updates the current attributes to the desired ones, writing the
    /// corresponding escape sequences to transition over.
    fn sync(&mut self, desired: &RunAttrs, color: bool, hyperlinks: bool, out: &mut String) {
        if color {
            write_style_diff(&mut self.styles, &desired.styles, out);
        }
        if hyperlinks {
            sync_link(&mut self.link, &desired.link, out);
        }
    }
}

/// A styled run of text.
struct Span {
    text: String,
    desired: RunAttrs,
}

/// A single word to be rendered.
#[derive(Default)]
struct Word {
    /// The sequence of styled spans that make up this word.
    spans: Vec<Span>,
    /// The current width of this word, in display columns.
    width: usize,
}

impl Word {
    fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    fn push(&mut self, text: &str, desired: &RunAttrs) {
        self.spans.push(Span {
            text: text.to_string(),
            desired: desired.clone(),
        });
        self.width += display_width(text);
    }

    fn take(&mut self) -> Vec<Span> {
        self.width = 0;
        std::mem::take(&mut self.spans)
    }
}

/// The set of styles for the help topic renderer.
#[derive(Default)]
struct Styles {
    heading: Style,
    emphasis: Style,
    strong: Style,
    strikethrough: Style,
    code: Style,
    term: Style,
    link: Style,
    toml_comment: Style,
    toml_table: Style,
    toml_string: Style,
    toml_number: Style,
    toml_boolean: Style,
}

impl Styles {
    fn colorize(&mut self) {
        self.heading = Style::new().bold().underline();
        self.emphasis = Style::new().italic();
        self.strong = Style::new().bold();
        self.strikethrough = Style::new().strikethrough();
        self.code = Style::new().cyan();
        self.term = Style::new().green().bold();
        self.link = Style::new().blue().underline();
        self.toml_comment = Style::new().dimmed();
        self.toml_table = Style::new().magenta().bold();
        self.toml_string = Style::new().green();
        self.toml_number = Style::new().yellow();
        self.toml_boolean = Style::new().yellow();
    }

    /// Maps synoptic's built-in toml token kinds onto our palette.
    fn toml_style(&self, kind: &str) -> Style {
        match kind {
            "comment" => self.toml_comment,
            "table" => self.toml_table,
            "string" => self.toml_string,
            "digit" | "keyword" => self.toml_number,
            "boolean" => self.toml_boolean,
            other => {
                // We're choosing to panic on an unknown kind because we're
                // targeting a small set of documents and we do snapshot tests
                // for all of them. (synoptic really should return a structured
                // enum rather than a string here!)
                //
                // Note that we use locked-tripwire to ensure cargo nextest
                // cannot be installed without --locked, so a newer version of
                // synoptic cannot come in and surprise us.
                panic!(
                    "unknown synoptic toml token kind {other:?} -- \
                     the built-in toml highlighter's token kinds may have changed"
                );
            }
        }
    }
}

/// The prefix written at the start of each line.
#[derive(Default)]
struct LinePrefix {
    // The stack of prefix segments, concatenated to form the prefix applied to
    // every line, unless the next line has an override.
    segments: SmallVec<[String; 4]>,
    // A one-shot override for the next line, if set.
    next_override: Option<String>,
}

impl LinePrefix {
    /// Pushes a prefix segment.
    fn push(&mut self, prefix: &str) {
        self.segments.push(prefix.to_string());
    }

    /// Pops the most recently pushed prefix segment.
    ///
    /// Panics if no segment has been pushed.
    fn pop(&mut self) {
        self.segments
            .pop()
            .expect("a prefix segment was pushed before this pop");
    }

    /// Sets an override for the next prefix by appending `marker` to the base
    /// prefix.
    fn set_next_override(&mut self, marker: &str) {
        let mut prefix = self.base();
        prefix.push_str(marker);
        self.next_override = Some(prefix);
    }

    /// Consumes the prefix for the next line.
    fn take_next(&mut self) -> String {
        self.next_override.take().unwrap_or_else(|| self.base())
    }

    fn base(&self) -> String {
        self.segments.concat()
    }
}

/// A line emitter.
///
/// This is the lower-level line-writing component. It is unaware of markdown.
struct LineWriter {
    out: String,
    color: bool,
    hyperlinks: bool,
    width: usize,

    prefix: LinePrefix,
    /// The state of the current line being written.
    line: LineState,

    /// The current word, plus a pending space that can act as a line break.
    word: Word,
    pending_space: Option<PendingSpace>,
    desired_attrs: RunAttrs,
}

/// A pending space that can also act as a line break.
///
/// Remembers the style and link currently active, so a space between two words
/// corresponding to the same link, e.g. the space between `a` and `b` in `[a
/// b](https://example.com)`, stays inside that link.
struct PendingSpace {
    desired: RunAttrs,
}

/// The mutable state of a line that is currently open.
#[derive(Clone, Debug)]
struct OpenLine {
    /// The number of visible columns used so far in this line.
    column: usize,
    /// The presentation emitted so far on this line.
    emitted: RunAttrs,
}

#[derive(Clone, Debug)]
enum LineState {
    /// The line has been opened: its prefix has been emitted, and content may
    /// follow.
    Open(OpenLine),
    /// The line is empty. Opening it writes the line prefix override if
    /// present, otherwise the prefix.
    AtStart,
    /// The line is empty.
    AfterBlock {
        /// The prefix to trim and write out on the blank line.
        prefix: String,
    },
}

impl LineWriter {
    fn new(color: bool, hyperlinks: bool, width: usize) -> Self {
        LineWriter {
            out: String::new(),
            color,
            hyperlinks,
            width,
            prefix: LinePrefix::default(),
            line: LineState::AtStart,
            word: Word::default(),
            pending_space: None,
            desired_attrs: RunAttrs::default(),
        }
    }

    fn push_style(&mut self, style: Style) {
        self.desired_attrs.styles.push(style);
    }

    fn pop_style(&mut self) {
        self.desired_attrs.styles.pop();
    }

    fn set_link(&mut self, link: LinkTarget) {
        self.desired_attrs.link = link;
    }

    fn take_link(&mut self) -> LinkTarget {
        std::mem::take(&mut self.desired_attrs.link)
    }

    /// Adds a styled segment into the current word.
    fn add_segment(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.word.push(text, &self.desired_attrs);
    }

    /// Adds a breakable space, styled by the surrounding context.
    fn add_space(&mut self) {
        self.flush_word();
        self.pending_space = Some(PendingSpace {
            desired: self.desired_attrs.clone(),
        });
    }

    /// Emits the buffered word, wrapping to the provided width with a hanging
    /// indent.
    fn flush_word(&mut self) {
        if self.word.is_empty() {
            return;
        }
        let pending = self.pending_space.take();
        match &self.line {
            LineState::Open(open) => {
                if let Some(space) = pending {
                    if open.column + 1 + self.word.width > self.width {
                        self.end_line();
                        self.open_line();
                    } else {
                        self.emit_space(&space.desired);
                    }
                }
            }
            LineState::AtStart | LineState::AfterBlock { .. } => self.open_line(),
        }
        self.emit_word();
    }

    fn emit_word(&mut self) {
        let spans = self.word.take();
        let Self {
            out,
            line,
            color,
            hyperlinks,
            ..
        } = self;
        let LineState::Open(open) = line else {
            return;
        };
        for span in spans {
            open.emitted.sync(&span.desired, *color, *hyperlinks, out);
            out.push_str(&span.text);
            open.column += display_width(&span.text);
        }
    }

    fn emit_space(&mut self, desired: &RunAttrs) {
        self.write_open(" ", desired);
    }

    /// Writes styled text to the current open line, syncing styles and advancing
    /// the column.
    fn write_open(&mut self, text: &str, desired: &RunAttrs) {
        let Self {
            out,
            line,
            color,
            hyperlinks,
            ..
        } = self;
        let LineState::Open(open) = line else {
            return;
        };
        open.emitted.sync(desired, *color, *hyperlinks, out);
        out.push_str(text);
        open.column += display_width(text);
    }

    /// Writes text verbatim (without wrapping), preserving newlines.
    ///
    /// Used for code blocks.
    fn verbatim(&mut self, s: &str) {
        // Code blocks are never hyperlinked, but preserve the current styles.
        let mut desired = self.desired_attrs.clone();
        desired.link = LinkTarget::Unlinked;
        self.for_each_verbatim_line(s, |w, _, line| w.write_open(line, &desired));
    }

    /// Writes verbatim text with TOML highlighting.
    fn verbatim_toml(&mut self, s: &str, palette: &Styles) {
        let lines: Vec<String> = s.split('\n').map(str::to_string).collect();
        let mut highlighter =
            from_extension("toml", 4).expect("synoptic provides a built-in toml highlighter");
        highlighter.run(&lines);

        self.for_each_verbatim_line(s, |w, i, line| {
            for token in highlighter.line(i, line) {
                match token {
                    TokOpt::Some(text, kind) => {
                        let desired = RunAttrs {
                            styles: StyleStack::from_slice(&[palette.toml_style(&kind)]),
                            link: LinkTarget::Unlinked,
                        };
                        w.write_open(&text, &desired);
                    }
                    TokOpt::None(text) => w.write_open(&text, &RunAttrs::default()),
                }
            }
        });
    }

    fn for_each_verbatim_line(
        &mut self,
        s: &str,
        mut write_line: impl FnMut(&mut Self, usize, &str),
    ) {
        let lines: Vec<&str> = s.split('\n').collect();
        let last = lines.len() - 1;
        for (i, &line) in lines.iter().enumerate() {
            if i != 0 {
                self.end_line();
            }
            if line.is_empty() {
                // If i is not the very last line, insert a line that the next
                // iteration will end (to insert a blank line). If i is the last
                // line, then don't append a trailing line.
                if i != last {
                    self.open_line();
                }
            } else {
                self.open_line();
                write_line(self, i, line);
            }
        }
    }

    fn hard_break(&mut self) {
        self.flush_word();
        self.end_line();
    }

    fn push_prefix(&mut self, prefix: &str) {
        self.prefix.push(prefix);
    }

    fn pop_prefix(&mut self) {
        self.prefix.pop();
    }

    fn set_item_marker(&mut self, marker: &str) {
        self.prefix.set_next_override(marker);
    }

    fn open_line(&mut self) {
        match &self.line {
            LineState::Open(_) => return,
            LineState::AtStart => {}
            LineState::AfterBlock { prefix } => {
                // Preserve the block-quote border across blank lines.
                if !self.out.is_empty() {
                    self.out.push_str(prefix.trim_end());
                    self.out.push('\n');
                }
            }
        }
        let prefix = self.prefix.take_next();
        self.out.push_str(&prefix);
        self.line = LineState::Open(OpenLine {
            column: display_width(&prefix),
            emitted: RunAttrs::default(),
        });
    }

    fn end_line(&mut self) {
        let LineState::Open(open) = &self.line else {
            return;
        };
        let reset = self.color && !open.emitted.styles.is_empty();
        let close_link = open.emitted.link.is_linked();
        // Drop trailing spaces so that blank and prefix-only lines don't carry
        // any whitespace. We use trim_end_matches rather than trim_end to avoid
        // eating newlines. If we used trim_end, we'd potentially end up
        // consuming the `\n` from the previous line.
        self.out.truncate(self.out.trim_end_matches(' ').len());
        if close_link {
            self.out.push_str("\x1b]8;;\x1b\\");
        }
        if reset {
            self.out.push_str(RESET_COLOR);
        }
        self.out.push('\n');
        self.line = LineState::AtStart;
    }

    fn end_block(&mut self) {
        self.end_line();
        self.line = LineState::AfterBlock {
            prefix: self.prefix.base(),
        };
    }

    /// Finishes the renderer, flushing any pending word and returning the
    /// accumulated output.
    fn finish(mut self) -> String {
        self.flush_word();
        self.end_line();
        self.out
    }
}

/// Diffs the currently-emitted stack of styles with the desired set of styles,
/// emitting the minimum number of style changes to match the target.
fn write_style_diff(emitted: &mut StyleStack, desired: &[Style], out: &mut String) {
    if emitted.as_slice() == desired {
        return;
    }
    let common = emitted.len();
    if desired.len() > common && &desired[..common] == emitted.as_slice() {
        for style in &desired[common..] {
            out.push_str(&style.prefix_formatter().to_string());
        }
    } else {
        if !emitted.is_empty() {
            out.push_str(RESET_COLOR);
        }
        for style in desired {
            out.push_str(&style.prefix_formatter().to_string());
        }
    }
    *emitted = StyleStack::from_slice(desired);
}

/// Syncs the current OSC 8 hyperlink with the desired link.
///
/// This is similar to `write_style_diff` above. The main purpose of this is to
/// prevent splitting hyperlinks on word boundaries.
fn sync_link(emitted: &mut LinkTarget, desired: &LinkTarget, out: &mut String) {
    if *emitted == *desired {
        return;
    }
    if emitted.is_linked() {
        out.push_str("\x1b]8;;\x1b\\");
    }
    if let LinkTarget::Linked(url) = desired {
        swrite!(out, "\x1b]8;;{url}\x1b\\");
    }
    *emitted = desired.clone();
}

struct Rendered {
    output: String,
    dropped: Vec<String>,
}

/// The current code block being rendered.
enum CodeBlockState {
    /// A code block without highlighting.
    Verbatim,
    /// A TOML code block.
    Toml { buffer: String },
}

fn render_markdown(
    markdown: &str,
    site_dir: &'static [&'static str],
    opts: RenderOptions,
) -> Rendered {
    let parser_options = Options::ENABLE_DEFINITION_LIST | Options::ENABLE_STRIKETHROUGH;
    let mut palette = Styles::default();
    if opts.color {
        palette.colorize();
    }
    let mut renderer = Renderer {
        writer: LineWriter::new(opts.color, opts.hyperlinks, opts.width),
        palette,
        site_dir,
        block: BlockContext::Normal(InlineContext::Normal),
        list_stack: Vec::new(),
        dropped: Vec::new(),
    };

    for event in Parser::new_ext(markdown, parser_options) {
        renderer.handle(event);
    }
    Rendered {
        output: renderer.writer.finish(),
        dropped: renderer.dropped,
    }
}

/// The current inline context the renderer is in.
#[derive(Clone, Copy)]
enum InlineContext {
    Normal,
    Term,
}

/// Block-level context for the renderer.
enum BlockContext {
    /// The normal block context, with no code block open.
    Normal(InlineContext),
    /// A code block is open.
    Code(CodeBlockState),
}

/// State maintained for a single open list.
struct ListFrame {
    /// The ordinal counter for ordered lists, or `None` for unordered lists.
    ordinal: Option<u64>,
}

/// A renderer that translates pulldown events into `LineWriter` operations.
struct Renderer {
    writer: LineWriter,
    palette: Styles,
    site_dir: &'static [&'static str],
    block: BlockContext,
    // The stack of open lists.
    list_stack: Vec<ListFrame>,
    dropped: Vec<String>,
}

impl Renderer {
    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                self.writer.flush_word();
                self.writer.end_block();
            }

            Event::Start(Tag::Heading { .. }) => {
                self.writer.end_block();
                self.writer.push_style(self.palette.heading);
            }
            Event::End(TagEnd::Heading(_)) => {
                self.writer.flush_word();
                self.writer.pop_style();
                self.writer.end_block();
            }

            Event::Start(Tag::BlockQuote(_)) => self.writer.push_prefix(QUOTE_PREFIX),
            Event::End(TagEnd::BlockQuote(_)) => {
                self.writer.flush_word();
                self.writer.pop_prefix();
                self.writer.end_block();
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                // Flush inline text buffered by a tight list item (e.g. `-
                // label:` before a fenced block), so the item marker stays on
                // the label, not the first code line.
                self.writer.flush_word();
                self.writer.end_line();
                self.writer.push_prefix(CODE_INDENT);
                self.block = BlockContext::Code(self.code_block_state_for(&kind));
                self.writer.push_style(self.palette.code);
            }
            Event::End(TagEnd::CodeBlock) => {
                // Reset the block context to normal, grabbing the TOML buffer
                // if it exists.
                if let BlockContext::Code(CodeBlockState::Toml { buffer }) =
                    std::mem::replace(&mut self.block, BlockContext::Normal(InlineContext::Normal))
                {
                    // (Note that non-TOML contexts were being rendered
                    // just-in-time, not buffered.)
                    self.writer.verbatim_toml(&buffer, &self.palette);
                }
                self.writer.pop_style();
                self.writer.pop_prefix();
                self.writer.end_block();
            }

            Event::Start(Tag::List(start)) => {
                self.list_stack.push(ListFrame { ordinal: start });
            }
            Event::End(TagEnd::List(_)) => {
                self.list_stack.pop();
                self.writer.end_block();
            }
            Event::Start(Tag::Item) => {
                // Flush any inline content buffered by the parent item before a
                // nested item starts. (This is relevant for tight nested lists
                // to avoid lines running into each other.)
                self.writer.flush_word();
                self.writer.end_line();
                let frame = self
                    .list_stack
                    .last_mut()
                    .expect("a list item is inside a list");
                let marker = match &mut frame.ordinal {
                    Some(index) => {
                        let marker = format!("{index}. ");
                        *index += 1;
                        marker
                    }
                    None => "- ".to_string(),
                };
                let indent = " ".repeat(display_width(&marker));
                self.writer.set_item_marker(&marker);
                self.writer.push_prefix(&indent);
            }
            Event::End(TagEnd::Item) => {
                self.writer.flush_word();
                self.writer.pop_prefix();
                self.writer.end_line();
            }

            Event::Start(Tag::DefinitionList) => {}
            Event::End(TagEnd::DefinitionList) => {}
            Event::Start(Tag::DefinitionListTitle) => {
                self.writer.end_block();
                self.writer.push_style(self.palette.term);
                self.block = BlockContext::Normal(InlineContext::Term);
            }
            Event::End(TagEnd::DefinitionListTitle) => {
                self.writer.flush_word();
                self.block = BlockContext::Normal(InlineContext::Normal);
                self.writer.pop_style();
                self.writer.end_line();
            }
            Event::Start(Tag::DefinitionListDefinition) => {
                self.writer.push_prefix(DEFINITION_INDENT);
            }
            Event::End(TagEnd::DefinitionListDefinition) => {
                self.writer.flush_word();
                self.writer.pop_prefix();
                self.writer.end_block();
            }

            Event::Start(Tag::Emphasis) => self.writer.push_style(self.palette.emphasis),
            Event::End(TagEnd::Emphasis) => self.writer.pop_style(),
            Event::Start(Tag::Strong) => self.writer.push_style(self.palette.strong),
            Event::End(TagEnd::Strong) => self.writer.pop_style(),
            Event::Start(Tag::Strikethrough) => self.writer.push_style(self.palette.strikethrough),
            Event::End(TagEnd::Strikethrough) => self.writer.pop_style(),

            Event::Start(Tag::Link { dest_url, .. }) => {
                // None means this is a same-page anchor -- drop it and render
                // text only.
                if let Some(url) = rewrite_url(&dest_url, self.site_dir) {
                    if self.writer.hyperlinks {
                        self.writer.push_style(self.palette.link);
                    }
                    self.writer.set_link(LinkTarget::Linked(url));
                }
            }
            Event::End(TagEnd::Link) => {
                // (None was a same-page anchor -- see Event::Start(Tag::Link {
                // .. }) above.)
                if let LinkTarget::Linked(url) = self.writer.take_link() {
                    if self.writer.hyperlinks {
                        self.writer.pop_style();
                    } else {
                        // The terminal doesn't support hyperlinks, so show the
                        // URL inline in the surrounding style.
                        self.writer.add_space();
                        self.writer.add_segment(&format!("<{url}>"));
                    }
                }
            }

            Event::Text(text) => match &mut self.block {
                BlockContext::Code(CodeBlockState::Toml { buffer }) => buffer.push_str(&text),
                BlockContext::Code(CodeBlockState::Verbatim) => self.writer.verbatim(&text),
                BlockContext::Normal(InlineContext::Normal | InlineContext::Term) => {
                    self.push_text(&text)
                }
            },
            Event::Code(code) => {
                match self.block {
                    BlockContext::Normal(InlineContext::Term) => {
                        // A definition term is already fully styled, so don't
                        // additionally style it with the code style. (Maybe we
                        // want to revisit this later?)
                        self.writer.add_segment(&code);
                    }
                    BlockContext::Normal(InlineContext::Normal) => {
                        self.writer.push_style(self.palette.code);
                        self.writer.add_segment(&code);
                        self.writer.pop_style();
                    }
                    BlockContext::Code(_) => {
                        unreachable!("inline code events cannot occur inside code blocks")
                    }
                }
            }
            Event::SoftBreak => self.writer.add_space(),
            Event::HardBreak => self.writer.hard_break(),

            // Ignore other events such as images, raw HTML, tables, footnotes,
            // etc. We may want to add support for these in the future, but
            // they're not necessary for the current set of things we render.
            other => self.dropped.push(format!("{other:?}")),
        }
    }

    /// Pushes text into the word buffer, splitting it into words and breakable
    /// spaces.
    fn push_text(&mut self, s: &str) {
        let mut run_start = 0;
        // A little state machine to track whether we're in a whitespace run.
        let mut run_is_ws: Option<bool> = None;
        for (i, c) in s.char_indices() {
            let ws = c.is_whitespace();
            match run_is_ws {
                None => {
                    run_is_ws = Some(ws);
                    run_start = i;
                }
                Some(prev) if prev == ws => {}
                Some(prev) => {
                    self.emit_run(&s[run_start..i], prev);
                    run_is_ws = Some(ws);
                    run_start = i;
                }
            }
        }
        if let Some(prev) = run_is_ws {
            self.emit_run(&s[run_start..], prev);
        }
    }

    fn emit_run(&mut self, run: &str, is_ws: bool) {
        if is_ws {
            self.writer.add_space();
        } else {
            self.writer.add_segment(run);
        }
    }

    fn code_block_state_for(&self, kind: &CodeBlockKind) -> CodeBlockState {
        if !self.writer.color {
            return CodeBlockState::Verbatim;
        }
        let CodeBlockKind::Fenced(info) = kind else {
            return CodeBlockState::Verbatim;
        };
        match info.split_whitespace().next() {
            Some("toml") => CodeBlockState::Toml {
                buffer: String::new(),
            },
            _ => CodeBlockState::Verbatim,
        }
    }
}

fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::core::NextestConfig;
    use nextest_filtering::FILTERSET_REFERENCE_MD;

    #[test]
    fn hyperlinks_gate_osc8() {
        let with = render(
            &HelpDoc::filterset(),
            RenderOptions {
                color: false,
                hyperlinks: true,
                width: 80,
            },
        );
        assert!(
            with.contains("\x1b]8;;https://nexte.st/"),
            "emits OSC-8 hyperlinks"
        );
        assert!(
            !with.contains("<https://nexte.st/"),
            "no inline URL fallback"
        );

        let without = render(
            &HelpDoc::filterset(),
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 80,
            },
        );
        assert!(!without.contains("\x1b]8;;"), "no OSC-8 when unsupported");
        assert!(
            without.contains("<https://nexte.st/"),
            "inline URL fallback"
        );
    }

    #[test]
    fn multi_word_link_is_one_continuous_hyperlink() {
        let doc = HelpDoc {
            markdown: "[click here](https://example.com/page) and more text\n".into(),
            site_dir: &[],
        };
        let rendered = render(
            &doc,
            RenderOptions {
                color: false,
                hyperlinks: true,
                width: 80,
            },
        );
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn wrapped_link_does_not_span_a_newline() {
        let doc = HelpDoc {
            markdown: "[alpha beta gamma](https://example.com/x)\n".into(),
            site_dir: &[],
        };
        let rendered = render(
            &doc,
            RenderOptions {
                color: false,
                hyperlinks: true,
                width: 12,
            },
        );
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn color_nested_styles() {
        let doc = HelpDoc {
            markdown: "**bold _and italic_** then `code`.\n".into(),
            site_dir: &[],
        };
        let rendered = render(
            &doc,
            RenderOptions {
                color: true,
                hyperlinks: false,
                width: 80,
            },
        );
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn ordered_list_indent_matches_marker_width() {
        let doc = HelpDoc {
            markdown: "1. aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii\n".into(),
            site_dir: &[],
        };
        let rendered = render(
            &doc,
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 20,
            },
        );
        let lines: Vec<&str> = rendered.lines().collect();
        assert!(lines[0].starts_with("1. "), "first line: {:?}", lines[0]);
        for cont in &lines[1..] {
            assert!(
                cont.starts_with("   ") && !cont.starts_with("    "),
                "continuation indented to marker width (3): {cont:?}"
            );
        }
    }

    #[test]
    fn nested_list_preserves_outer_ordinal() {
        let doc = HelpDoc {
            markdown: "1. first\n2. second\n   - nested a\n   - nested b\n3. third\n".into(),
            site_dir: &[],
        };
        let rendered = render(
            &doc,
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 80,
            },
        );
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn code_block_preserves_blank_lines() {
        let doc = HelpDoc {
            markdown: "```\nfirst\n\nsecond\n```\n".into(),
            site_dir: &[],
        };
        let rendered = render(
            &doc,
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 80,
            },
        );
        assert!(
            rendered.contains("first\n\n") && rendered.contains("second"),
            "interior blank line preserved: {rendered:?}"
        );
    }

    #[test]
    fn list_item_label_before_code_block_keeps_marker_on_label() {
        let doc = HelpDoc {
            markdown: "- **Examples**:\n  ```\n  retries = 3\n  ```\n".into(),
            site_dir: &[],
        };
        let rendered = render(
            &doc,
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 80,
            },
        );
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn blockquote_code_block_blank_line_keeps_border() {
        let doc = HelpDoc {
            markdown: "> ```\n> a\n>\n> b\n> ```\n".into(),
            site_dir: &[],
        };
        let rendered = render(
            &doc,
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 80,
            },
        );
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn admonition_with_unicode_indent_does_not_panic() {
        let doc = HelpDoc {
            markdown: "!!! note\n   \u{a0}body text\n".into(),
            site_dir: &[],
        };
        let _ = render(
            &doc,
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 80,
            },
        );
    }

    #[test]
    fn rewrite_url_resolves_relative_links() {
        let site_dir = HelpDoc::filterset().site_dir;
        assert_eq!(
            rewrite_url("https://example.com/x", site_dir).as_deref(),
            Some("https://example.com/x"),
        );
        assert_eq!(
            rewrite_url("http://example.com/x", site_dir).as_deref(),
            Some("http://example.com/x"),
        );
        assert_eq!(rewrite_url("#binary-kinds", site_dir), None);
        assert_eq!(
            rewrite_url("../glossary.md#binary-id", site_dir).as_deref(),
            Some("https://nexte.st/docs/glossary/#binary-id"),
        );
        assert_eq!(
            rewrite_url("../configuration/test-groups.md", site_dir).as_deref(),
            Some("https://nexte.st/docs/configuration/test-groups/"),
        );
        assert_eq!(
            rewrite_url("../selecting.md", site_dir).as_deref(),
            Some("https://nexte.st/docs/selecting/"),
        );

        // index.md resolves to the corresponding directory URL.
        let config_dir: &[&str] = &["docs", "configuration"];
        assert_eq!(
            rewrite_url("index.md", config_dir).as_deref(),
            Some("https://nexte.st/docs/configuration/"),
        );
        assert_eq!(
            rewrite_url("index.md#hierarchical-configuration", config_dir).as_deref(),
            Some("https://nexte.st/docs/configuration/#hierarchical-configuration"),
        );
        assert_eq!(
            rewrite_url("../user-config/index.md", config_dir).as_deref(),
            Some("https://nexte.st/docs/user-config/"),
        );
        assert_eq!(
            rewrite_url("../filtersets/index.md", config_dir).as_deref(),
            Some("https://nexte.st/docs/filtersets/"),
        );
    }

    #[test]
    #[should_panic(expected = "relative link in help doc escapes the site root")]
    fn rewrite_url_panics_on_escaping_links() {
        let site_dir = HelpDoc::filterset().site_dir;
        rewrite_url("../../../../foo.md", site_dir);
    }

    #[test]
    fn apply_shortcodes_handles_versions_and_comments() {
        assert_eq!(
            apply_shortcodes("a <!-- md:version 0.9.1 --> b"),
            "a *(since 0.9.1)* b",
        );
        assert_eq!(apply_shortcodes("a <!-- md:version --> b"), "a  b");
        assert_eq!(apply_shortcodes("a <!-- md:versionfoo --> b"), "a  b");
        assert_eq!(apply_shortcodes("a <!-- random comment --> b"), "a  b");
        assert_eq!(
            apply_shortcodes("a <!-- unterminated"),
            "a <!-- unterminated"
        );
    }

    #[test]
    fn reference_stays_within_supported_subset() {
        let markdown = FILTERSET_REFERENCE_MD;

        let mut rest = markdown;
        while let Some(idx) = rest.find("<!-- md:") {
            let after = &rest[idx + "<!-- md:".len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            assert_eq!(
                name, "version",
                "unsupported `md:{name}` shortcode in the filterset reference; \
                 the CLI help renderer only handles `md:version` (see apply_shortcodes)"
            );
            rest = after;
        }

        let preprocessed = preprocess(markdown);
        let rendered = render_markdown(
            &preprocessed,
            HelpDoc::filterset().site_dir,
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 80,
            },
        );
        assert!(
            rendered.dropped.is_empty(),
            "filterset reference uses markdown the CLI help renderer silently drops: {:?}",
            rendered.dropped,
        );
    }

    #[test]
    fn repo_config_reference_stays_within_supported_subset() {
        let markdown = NextestConfig::REFERENCE_MD;

        let mut rest = markdown;
        while let Some(idx) = rest.find("<!-- md:") {
            let after = &rest[idx + "<!-- md:".len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            assert_eq!(
                name, "version",
                "unsupported `md:{name}` shortcode in the repo-config reference; \
                 the CLI help renderer only handles `md:version` (see apply_shortcodes)"
            );
            rest = after;
        }

        let preprocessed = preprocess(markdown);
        let rendered = render_markdown(
            &preprocessed,
            &["docs", "configuration"],
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 80,
            },
        );
        assert!(
            rendered.dropped.is_empty(),
            "repo-config reference uses markdown the CLI help renderer silently drops: {:?}",
            rendered.dropped,
        );
    }

    #[test]
    fn repo_config_markdown_includes_reference_and_defaults() {
        let markdown = repo_config_markdown();
        assert!(
            markdown.contains("# Configuration reference"),
            "repo-config topic includes the reference body"
        );
        assert!(
            markdown.contains("## Default configuration"),
            "repo-config topic appends a default configuration section"
        );
        assert!(
            markdown.contains(NextestConfig::DEFAULT_CONFIG.trim_end()),
            "repo-config topic embeds the default config verbatim"
        );
    }

    #[test]
    fn user_config_reference_stays_within_supported_subset() {
        let markdown = UserConfig::REFERENCE_MD;

        let mut rest = markdown;
        while let Some(idx) = rest.find("<!-- md:") {
            let after = &rest[idx + "<!-- md:".len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            assert_eq!(
                name, "version",
                "unsupported `md:{name}` shortcode in the user-config reference; \
                 the CLI help renderer only handles `md:version` (see apply_shortcodes)"
            );
            rest = after;
        }

        let preprocessed = preprocess(&user_config_markdown());
        let rendered = render_markdown(
            &preprocessed,
            &["docs", "user-config"],
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 80,
            },
        );
        assert!(
            rendered.dropped.is_empty(),
            "user-config reference uses markdown the CLI help renderer silently drops: {:?}",
            rendered.dropped,
        );
    }

    #[test]
    fn user_config_markdown_includes_reference_and_defaults() {
        let markdown = user_config_markdown();
        assert!(
            markdown.contains("# User config reference"),
            "user-config topic includes the reference body"
        );
        assert!(
            markdown.contains("## Default configuration"),
            "user-config topic appends a default configuration section"
        );
        assert!(
            markdown.contains(UserConfig::DEFAULT_CONFIG.trim_end()),
            "user-config topic embeds the default config verbatim"
        );
    }

    #[test]
    fn toml_code_block_is_highlighted() {
        let doc = HelpDoc {
            markdown: "```toml\n[profile.ci]\n# run all tests\nfail-fast = false\nretries = 3\nslow-timeout = \"60s\"\n```\n".into(),
            site_dir: &[],
        };
        let colored = render(
            &doc,
            RenderOptions {
                color: true,
                hyperlinks: false,
                width: 80,
            },
        );
        insta::assert_snapshot!("toml_highlight_color", colored);

        let plain = render(
            &doc,
            RenderOptions {
                color: false,
                hyperlinks: false,
                width: 80,
            },
        );
        assert!(
            !plain.contains('\x1b'),
            "a toml block emits no ANSI when color is off: {plain:?}"
        );
    }
}
