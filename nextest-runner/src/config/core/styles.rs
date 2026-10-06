// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use owo_colors::Style;

/// Terminal styles for configuration-related diagnostics.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConfigStyles {
    /// Style for a config file path.
    pub path: Style,
    /// Style for the name of the tool that provided a config file.
    pub tool: Style,
}

impl ConfigStyles {
    /// Colorizes the styles for terminal output.
    pub fn colorize(&mut self) {
        self.path = Style::new().cyan();
        self.tool = Style::new().bold().yellow();
    }
}
