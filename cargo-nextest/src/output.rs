// Copyright (c) The nextest Contributors
// Copyright (c) The cargo-guppy Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use clap::{Args, ValueEnum};
use miette::{GraphicalTheme, MietteHandlerOpts, ThemeStyles};
use nextest_runner::{reporter::ReporterStderr, write_str::WriteStr};
use owo_colors::{style, OwoColorize, Style};
use std::{
    fmt,
    io::{self, BufWriter, Stderr, Stdout, Write},
    marker::PhantomData,
};
use tracing::{
    field::{Field, Visit},
    level_filters::LevelFilter,
    Event, Level, Subscriber,
};
use tracing_subscriber::{
    filter::Targets,
    fmt::{format, FmtContext, FormatEvent, FormatFields},
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
    Layer,
};

pub(crate) mod clap_styles {
    use clap::builder::{
        styling::{AnsiColor, Effects, Style},
        Styles,
    };

    const HEADER: Style = AnsiColor::Green.on_default().effects(Effects::BOLD);
    const USAGE: Style = AnsiColor::Green.on_default().effects(Effects::BOLD);
    const LITERAL: Style = AnsiColor::Cyan.on_default().effects(Effects::BOLD);
    const PLACEHOLDER: Style = AnsiColor::Cyan.on_default();
    const ERROR: Style = AnsiColor::Red.on_default().effects(Effects::BOLD);
    const VALID: Style = AnsiColor::Cyan.on_default().effects(Effects::BOLD);
    const INVALID: Style = AnsiColor::Yellow.on_default().effects(Effects::BOLD);

    pub(crate) const fn style() -> Styles {
        // Copied from
        // https://github.com/rust-lang/cargo/blob/98f6bf3700e2918678acd87b7e1a1450df579853/src/bin/cargo/cli.rs#L552-L561
        // to match Cargo's style.
        Styles::styled()
            .header(HEADER)
            .usage(USAGE)
            .literal(LITERAL)
            .placeholder(PLACEHOLDER)
            .error(ERROR)
            .valid(VALID)
            .invalid(INVALID)
    }
}

#[derive(Copy, Clone, Debug, Args)]
#[must_use]
pub(crate) struct OutputOpts {
    /// Verbose output
    #[arg(long, short, global = true, env = "NEXTEST_VERBOSE")]
    pub(crate) verbose: bool,
    // TODO: quiet?
    /// Produce color output: auto, always, never
    #[arg(
        long,
        value_enum,
        default_value_t,
        hide_possible_values = true,
        global = true,
        value_name = "WHEN",
        env = "CARGO_TERM_COLOR"
    )]
    pub(crate) color: Color,
}

impl OutputOpts {
    pub(crate) fn init(self) -> OutputContext {
        let OutputOpts { verbose, color } = self;

        color.init();

        OutputContext { verbose, color }
    }
}

#[derive(Copy, Clone, Debug)]
#[must_use]
pub struct OutputContext {
    pub(crate) verbose: bool,
    pub(crate) color: Color,
}

impl OutputContext {
    // color_never_init is only used for double-spawning, which only exists on Unix platforms.
    #[cfg(unix)]
    pub(crate) fn color_never_init() -> Self {
        Color::Never.init();
        Self {
            verbose: false,
            color: Color::Never,
        }
    }

    /// Returns general stderr styles for the current output context.
    pub fn stderr_styles(&self) -> StderrStyles {
        let mut styles = StderrStyles::default();

        if self.color.should_colorize(supports_color::Stream::Stderr) {
            styles.colorize();
        }

        styles
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[must_use]
#[derive(Default)]
pub enum Color {
    #[default]
    Auto,
    Always,
    Never,
}

static INIT_LOGGER: std::sync::Once = std::sync::Once::new();

struct SimpleFormatter {
    styles: LogStyles,
}

impl<S, N> FormatEvent<S, N> for SimpleFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: format::Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();

        if metadata.target() != "cargo_nextest::no_heading" {
            match *metadata.level() {
                Level::ERROR => {
                    write!(writer, "{}: ", "error".style(self.styles.error))?;
                }
                Level::WARN => {
                    write!(writer, "{}: ", "warning".style(self.styles.warning))?;
                }
                Level::INFO => {
                    write!(writer, "{}: ", "info".style(self.styles.info))?;
                }
                Level::DEBUG => {
                    write!(writer, "{}: ", "debug".style(self.styles.debug))?;
                }
                Level::TRACE => {
                    write!(writer, "{}: ", "trace".style(self.styles.trace))?;
                }
            }
        }

        let mut visitor = MessageVisitor {
            writer: &mut writer,
            error: None,
        };

        event.record(&mut visitor);

        if let Some(error) = visitor.error {
            return Err(error);
        }

        writeln!(writer)
    }
}

static MESSAGE_FIELD: &str = "message";

struct MessageVisitor<'writer, 'a> {
    writer: &'a mut format::Writer<'writer>,
    error: Option<fmt::Error>,
}

impl<'writer, 'a> Visit for MessageVisitor<'writer, 'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == MESSAGE_FIELD {
            if let Err(error) = write!(self.writer, "{:?}", value) {
                self.error = Some(error);
            }
        }
    }
}

impl Color {
    pub(crate) fn init(self) {
        // Pass the styles in as a stylesheet to ensure we use the latest supports-color here.
        let mut log_styles = LogStyles::default();
        if self.should_colorize(supports_color::Stream::Stderr) {
            log_styles.colorize();
        }

        INIT_LOGGER.call_once(|| {
            let level_str = std::env::var_os("NEXTEST_LOG").unwrap_or_default();
            let level_str = level_str
                .into_string()
                .unwrap_or_else(|_| panic!("NEXTEST_LOG is not UTF-8"));

            // If the level string is empty, use the standard level filter instead.
            let targets = if level_str.is_empty() {
                Targets::new().with_default(LevelFilter::INFO)
            } else {
                level_str.parse().expect("unable to parse NEXTEST_LOG")
            };

            let layer = tracing_subscriber::fmt::layer()
                .event_format(SimpleFormatter { styles: log_styles })
                .with_writer(std::io::stderr)
                .with_filter(targets);

            tracing_subscriber::registry().with(layer).init();

            miette::set_hook(Box::new(move |_| {
                let theme_styles = if self.should_colorize(supports_color::Stream::Stderr) {
                    ThemeStyles {
                        error: style().red().bold(),
                        warning: style().yellow().bold(),
                        advice: style().bright_cyan().bold(),
                        help: style().cyan(),
                        link: style().cyan().underline().bold(),
                        linum: style().dimmed(),
                        highlights: vec![style().red(), style().yellow(), style().bright_cyan()],
                    }
                } else {
                    ThemeStyles::none()
                };
                let mut graphical_theme = if supports_unicode::on(supports_unicode::Stream::Stderr)
                {
                    GraphicalTheme::unicode()
                } else {
                    GraphicalTheme::ascii()
                };
                graphical_theme.characters.error = "error:".into();
                graphical_theme.styles = theme_styles;

                let handler = MietteHandlerOpts::new().graphical_theme(graphical_theme);
                Box::new(handler.build())
            }))
            .expect("miette::set_hook should only be called once");
        });
    }

    pub(crate) fn should_colorize(self, stream: supports_color::Stream) -> bool {
        match self {
            Color::Auto => supports_color::on_cached(stream).is_some(),
            Color::Always => true,
            Color::Never => false,
        }
    }

    pub(crate) fn to_arg(self) -> &'static str {
        match self {
            Color::Auto => "--color=auto",
            Color::Always => "--color=always",
            Color::Never => "--color=never",
        }
    }
}

#[derive(Debug, Default)]
struct LogStyles {
    error: Style,
    warning: Style,
    info: Style,
    debug: Style,
    trace: Style,
}

impl LogStyles {
    fn colorize(&mut self) {
        self.error = style().red().bold();
        self.warning = style().yellow().bold();
        self.info = style().bold();
        self.debug = style().bold();
        self.trace = style().dimmed();
    }
}

#[derive(Debug, Default)]
pub struct StderrStyles {
    pub(crate) bold: Style,
    pub(crate) warning_text: Style,
}

impl StderrStyles {
    fn colorize(&mut self) {
        self.bold = style().bold();
        self.warning_text = style().yellow();
    }
}

/// A helper for capturing output in tests
///
/// The test pass is gated by `#[cfg(test)]` to allow a better
/// optimization in the binary.
pub enum OutputWriter {
    /// No capture
    Normal,
    /// Output captured
    #[cfg(test)]
    Test {
        /// stdout capture
        stdout: Vec<u8>,
        /// stderr capture
        stderr: Vec<u8>,
    },
}

impl Default for OutputWriter {
    fn default() -> Self {
        Self::Normal
    }
}

impl OutputWriter {
    pub(crate) fn stdout_writer(&mut self) -> StdoutWriter<'_> {
        match self {
            Self::Normal => StdoutWriter::Normal {
                buf: BufWriter::new(std::io::stdout()),
                _lifetime: PhantomData,
            },
            #[cfg(test)]
            Self::Test { ref mut stdout, .. } => StdoutWriter::Test { buf: stdout },
        }
    }

    pub(crate) fn reporter_output(&mut self) -> ReporterStderr<'_> {
        match self {
            Self::Normal => ReporterStderr::Terminal,
            #[cfg(test)]
            Self::Test { ref mut stderr, .. } => ReporterStderr::Buffer(stderr),
        }
    }

    pub(crate) fn stderr_writer(&mut self) -> StderrWriter<'_> {
        match self {
            Self::Normal => StderrWriter::Normal {
                buf: BufWriter::new(std::io::stderr()),
                _lifetime: PhantomData,
            },
            #[cfg(test)]
            Self::Test { ref mut stderr, .. } => StderrWriter::Test { buf: stderr },
        }
    }
}

pub(crate) enum StdoutWriter<'a> {
    Normal {
        buf: BufWriter<Stdout>,
        _lifetime: PhantomData<&'a ()>,
    },
    #[cfg(test)]
    Test { buf: &'a mut Vec<u8> },
}

impl<'a> Write for StdoutWriter<'a> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Normal { buf, .. } => buf.write(data),
            #[cfg(test)]
            Self::Test { buf } => buf.write(data),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.flush(),
            #[cfg(test)]
            Self::Test { .. } => Ok(()),
        }
    }
}

impl<'a> WriteStr for StdoutWriter<'a> {
    fn write_str(&mut self, s: &str) -> io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.write_all(s.as_bytes()),
            #[cfg(test)]
            Self::Test { buf } => buf.write_all(s.as_bytes()),
        }
    }

    fn write_str_flush(&mut self) -> io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.flush(),
            #[cfg(test)]
            Self::Test { .. } => Ok(()),
        }
    }
}

pub(crate) enum StderrWriter<'a> {
    Normal {
        buf: BufWriter<Stderr>,
        _lifetime: PhantomData<&'a ()>,
    },
    #[cfg(test)]
    Test { buf: &'a mut Vec<u8> },
}

impl<'a> Write for StderrWriter<'a> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Normal { buf, .. } => buf.write(data),
            #[cfg(test)]
            Self::Test { buf } => buf.write(data),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.flush(),
            #[cfg(test)]
            Self::Test { .. } => Ok(()),
        }
    }
}

pub(crate) fn should_redact() -> bool {
    std::env::var("__NEXTEST_REDACT") == Ok("1".to_string())
}
