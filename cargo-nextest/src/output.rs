// Copyright (c) The nextest Contributors
// Copyright (c) The cargo-guppy Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::dispatch::EarlyArgs;
use clap::{Args, ValueEnum};
use miette::{GraphicalTheme, MietteHandlerOpts, ThemeStyles};
use nextest_runner::{reporter::ReporterOutput, write_str::WriteStr};
use owo_colors::{OwoColorize, Style, style};
use std::{
    fmt,
    io::{self, BufWriter, Stderr, Stdout, Write},
    marker::PhantomData,
};
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
    level_filters::LevelFilter,
};
use tracing_subscriber::{
    Layer,
    filter::Targets,
    fmt::{FmtContext, FormatEvent, FormatFields, format},
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
};

pub(crate) mod clap_styles {
    use clap::builder::{
        Styles,
        styling::{AnsiColor, Effects, Style},
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
}

impl OutputOpts {
    pub(crate) fn init(self, early_args: &EarlyArgs) -> OutputContext {
        let OutputOpts { verbose } = self;

        early_args.color.init();

        OutputContext {
            verbose,
            color: early_args.color,
        }
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
            // Show other fields for debug or trace output.
            show_other: *metadata.level() >= Level::DEBUG,
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
    show_other: bool,
    error: Option<fmt::Error>,
}

impl Visit for MessageVisitor<'_, '_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == MESSAGE_FIELD {
            if let Err(error) = write!(self.writer, "{value:?}") {
                self.error = Some(error);
            }
        } else if self.show_other
            && let Err(error) = write!(self.writer, "; {} = {:?}", field.name(), value)
        {
            self.error = Some(error);
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

            cfg_if::cfg_if! {
                if #[cfg(feature = "experimental-tokio-console")] {
                    let console_layer = nextest_runner::console::spawn();
                    tracing_subscriber::registry()
                        .with(layer)
                        .with(console_layer)
                        .init();
                } else {
                    tracing_subscriber::registry()
                        .with(layer)
                        .init();
                }
            }

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
    pub(crate) list_styles: nextest_runner::list::Styles,
    pub(crate) record_styles: nextest_runner::record::Styles,
}

impl StderrStyles {
    fn colorize(&mut self) {
        self.bold = style().bold();
        self.warning_text = style().yellow();
        self.list_styles.colorize();
        self.record_styles.colorize();
    }
}

/// A helper for capturing output in tests
///
/// The test pass is gated by `#[cfg(test)]` to allow a better
/// optimization in the binary.
#[derive(Default)]
pub enum OutputWriter {
    /// No capture
    #[default]
    Normal,
}

impl OutputWriter {
    pub(crate) fn stdout_writer(&mut self) -> StdoutWriter<'_> {
        match self {
            Self::Normal => StdoutWriter::Normal {
                buf: BufWriter::new(std::io::stdout()),
                _lifetime: PhantomData,
            },
        }
    }

    pub(crate) fn reporter_output(&mut self) -> ReporterOutput<'_> {
        match self {
            Self::Normal => ReporterOutput::Terminal,
        }
    }

    pub(crate) fn stderr_writer(&mut self) -> StderrWriter<'_> {
        match self {
            Self::Normal => StderrWriter::Normal {
                buf: BufWriter::new(std::io::stderr()),
                _lifetime: PhantomData,
            },
        }
    }
}

pub(crate) enum StdoutWriter<'a> {
    Normal {
        buf: BufWriter<Stdout>,
        _lifetime: PhantomData<&'a ()>,
    },
}

impl WriteStr for StdoutWriter<'_> {
    fn write_str(&mut self, s: &str) -> io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.write_str(s),
        }
    }

    fn write_str_flush(&mut self) -> io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.flush(),
        }
    }
}

pub(crate) enum StderrWriter<'a> {
    Normal {
        buf: BufWriter<Stderr>,
        _lifetime: PhantomData<&'a ()>,
    },
}

impl WriteStr for StderrWriter<'_> {
    fn write_str(&mut self, s: &str) -> io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.write_str(s),
        }
    }

    fn write_str_flush(&mut self) -> io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.flush(),
        }
    }
}

pub(crate) fn should_redact() -> bool {
    std::env::var("__NEXTEST_REDACT") == Ok("1".to_string())
}
