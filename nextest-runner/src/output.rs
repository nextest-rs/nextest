// Copyright (c) The nextest Contributors
// Copyright (c) The cargo-guppy Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration of nextest output, such as colorization and style

use std::fmt;
use clap::ValueEnum;
use miette::{GraphicalTheme, MietteHandlerOpts, ThemeStyles};
use owo_colors::{OwoColorize, style, Style};
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

/// High level specification of nextest output options
#[derive(Copy, Clone, Debug)]
#[must_use]
pub struct OutputContext {
    /// Request the output to be verbose
    pub verbose: bool,

    /// Specify how colorization is determined (not what color is used)
    pub color: Color,
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

/// Specifies whether to colorize output
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[must_use]
#[derive(Default)]
pub enum Color {
    /// Determine coloration based on whether the actual terminal supports it and whether the 'NO_COLOR' environment variable is
    #[default]
    Auto,

    /// Always try to colorize
    Always,

    /// Never try to colorize
    Never,
}

impl Color {
    /// Initialize color-related hooks and logging
    pub fn init(self) {
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

    /// Determines whether output should be colorized based on whether the given stream supports this
    pub fn should_colorize(self, stream: supports_color::Stream) -> bool {
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

/// Determines how to style stderr output
#[derive(Debug, Default)]
pub struct StderrStyles {
    /// The style for 'bold' output
    pub bold: Style,

    /// The style for 'warning' output
    pub warning_text: Style,
}

impl StderrStyles {
    fn colorize(&mut self) {
        self.bold = style().bold();
        self.warning_text = style().yellow();
    }
}

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
        } else if self.show_other {
            if let Err(error) = write!(self.writer, "; {} = {:?}", field.name(), value) {
                self.error = Some(error);
            }
        }
    }
}

static INIT_LOGGER: std::sync::Once = std::sync::Once::new();

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