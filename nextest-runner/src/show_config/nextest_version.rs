// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    config::core::{NextestVersionConfig, NextestVersionEval, NextestVersionReq},
    write_str::WriteStr,
};
use owo_colors::{OwoColorize, Style};
use semver::Version;
use std::io;

/// Show version-related configuration.
pub struct ShowNextestVersion<'a> {
    version_cfg: &'a NextestVersionConfig,
    current_version: &'a Version,
    override_version_check: bool,
}

impl<'a> ShowNextestVersion<'a> {
    /// Construct a new [`ShowNextestVersion`].
    pub fn new(
        version_cfg: &'a NextestVersionConfig,
        current_version: &'a Version,
        override_version_check: bool,
    ) -> Self {
        Self {
            version_cfg,
            current_version,
            override_version_check,
        }
    }

    /// Write the version configuration in human-readable form.
    pub fn write_human(&self, writer: &mut dyn WriteStr, colorize: bool) -> io::Result<()> {
        let mut styles = Styles::default();
        if colorize {
            styles.colorize();
        }

        writeln!(
            writer,
            "current nextest version: {}",
            self.current_version.style(styles.version)
        )?;

        write!(writer, "version requirements:")?;

        let mut any_requirements = false;
        if let NextestVersionReq::Version { version, tool } = &self.version_cfg.required {
            if !any_requirements {
                writeln!(writer)?;
            }
            any_requirements = true;
            write!(writer, "    - required: {}", version.style(styles.version))?;
            if let Some(tool) = tool {
                writeln!(writer, " (by tool {})", tool.style(styles.tool))?;
            } else {
                writeln!(writer)?;
            }
        }

        if let NextestVersionReq::Version { version, tool } = &self.version_cfg.recommended {
            if !any_requirements {
                writeln!(writer)?;
            }
            any_requirements = true;
            write!(
                writer,
                "    - recommended: {}",
                version.style(styles.version)
            )?;
            if let Some(tool) = tool {
                writeln!(writer, " (by tool {})", tool.style(styles.tool))?;
            } else {
                writeln!(writer)?;
            }
        }

        if any_requirements {
            write!(writer, "evaluation result: ")?;
            let eval = self
                .version_cfg
                .eval(self.current_version, self.override_version_check);
            match eval {
                NextestVersionEval::Satisfied => {
                    writeln!(writer, "{}", "ok".style(styles.satisfied))?;
                }
                NextestVersionEval::Error { .. } => {
                    writeln!(
                        writer,
                        "{}",
                        "does not meet required version".style(styles.error)
                    )?;
                }
                NextestVersionEval::Warn { .. } => {
                    writeln!(
                        writer,
                        "{}",
                        "does not meet recommended version".style(styles.warning)
                    )?;
                }
                NextestVersionEval::ErrorOverride { .. } => {
                    writeln!(
                        writer,
                        "does not meet required version, but is {}",
                        "overridden".style(styles.overridden),
                    )?;
                }
                crate::config::core::NextestVersionEval::WarnOverride { .. } => {
                    writeln!(
                        writer,
                        "does not meet recommended version, but is {}",
                        "overridden".style(styles.overridden),
                    )?;
                }
            }
        } else {
            writeln!(writer, " (none)")?;
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
struct Styles {
    version: Style,
    tool: Style,
    satisfied: Style,
    error: Style,
    warning: Style,
    overridden: Style,
}

impl Styles {
    fn colorize(&mut self) {
        self.version = Style::new().bold();
        self.tool = Style::new().bold().yellow();
        self.satisfied = Style::new().bold().green();
        self.error = Style::new().bold().red();
        self.warning = Style::new().bold().yellow();
        self.overridden = Style::new().bold();
    }
}
