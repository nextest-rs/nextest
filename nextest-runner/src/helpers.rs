// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::list::Styles;
use owo_colors::OwoColorize;
use std::io::{self, Write};

/// Write out a test name.
pub(crate) fn write_test_name(
    name: &str,
    style: &Styles,
    mut writer: impl Write,
) -> io::Result<()> {
    // Look for the part of the test after the last ::, if any.
    let mut splits = name.rsplitn(2, "::");
    let trailing = splits.next().expect("test should have at least 1 element");
    if let Some(rest) = splits.next() {
        write!(
            writer,
            "{}{}",
            rest.style(style.module_path),
            "::".style(style.module_path)
        )?;
    }
    write!(writer, "{}", trailing.style(style.test_name))?;

    Ok(())
}
