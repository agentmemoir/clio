// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the styled fragments `clio setup` prints:
//! every string that reaches a terminal with colour applied, and the fixed lines
//! a run prints when it goes back or quits.
//!
//! ## Owns
//! - The styled fragments: title, section, default hint, confirmation, warning, error
//! - The going-back, quitting, and nothing-to-undo lines
//! - The two `Style` constructors the fragments share
//!
//! ## Does not own
//! - Which questions exist and in what order (see `setup_steps`)
//! - How a prompt is written and read (see `setup_render`)
//! - Whether colour is available at all (see `cli_style`)
//!
//! ## Boundary
//!
//! Changes to what a fragment says, or to the colour it wears, should normally
//! require changes here. Changes to which questions are asked, or to how an
//! answer is read, should not require changes here.
//!
//! Keep this module focused on producing styled text with no input and no output
//! stream of its own.
//!
//! # Why colour-off output must stay byte-identical
//!
//! `Palette::paint` returns its input untouched when colour is off, so a piped
//! run, a `NO_COLOR` run, and every golden test see plain text. The golden tests
//! for the fragments this module keeps are what prove that a restyle did not
//! silently move a byte that something downstream parses.
//!
//! # Why these are not in `setup_render`
//!
//! The fragments take a `Palette` and a string and return a string. They read no
//! line and write no prompt, so they are a different job from the one
//! `setup_render` does, and splitting them keeps both files under the size limit
//! without either becoming a grab bag.

use crate::cli_style::Palette;
use anstyle::{AnsiColor, Style};

/// The wizard title, printed once at the top of a run.
#[must_use]
pub(crate) fn title(palette: Palette) -> String {
    palette.paint(Style::new().bold(), "Clio setup")
}

/// A section heading, dim so the questions under it stand out.
#[must_use]
pub(crate) fn section(palette: Palette, name: &str) -> String {
    palette.paint(dim(), name)
}

/// A default value, dim and in brackets so it reads as a suggestion.
#[must_use]
pub(crate) fn hint(palette: Palette, default: &str) -> String {
    palette.paint(dim(), &format!("[{default}]"))
}

/// An explanation under a menu option, dim and unbracketed.
///
/// `hint` is for a value the operator can accept; this is prose explaining a
/// choice, and surrounding it with brackets would read as another value.
#[must_use]
pub(crate) fn detail(palette: Palette, text: &str) -> String {
    palette.paint(dim(), text)
}

/// Something that worked.
#[must_use]
pub(crate) fn confirm(palette: Palette, text: &str) -> String {
    palette.paint(colour(AnsiColor::Green), text)
}

/// Something an operator should read before agreeing.
#[must_use]
pub(crate) fn warn(palette: Palette, text: &str) -> String {
    palette.paint(colour(AnsiColor::Yellow), text)
}

/// Something that refused.
#[must_use]
pub(crate) fn error(palette: Palette, text: &str) -> String {
    palette.paint(colour(AnsiColor::Red), text)
}

/// The line printed when a question is abandoned.
#[must_use]
pub(crate) fn quitting() -> &'static str {
    "Nothing has been started and nothing has been written."
}

/// The second line of the going-back block: why going back is free.
#[must_use]
pub(crate) fn nothing_to_undo() -> &'static str {
    "Nothing has been started and nothing has been written yet."
}

/// The line printed before a discarded answer is dropped, by section name.
///
/// The section is named *before* it goes, so the choice is informed rather than
/// discovered afterwards.
#[must_use]
pub(crate) fn going_back(section: &str) -> String {
    format!("Going back. Your answers to {section} will be discarded.")
}

/// The dim style the heading and the default hint share.
fn dim() -> Style {
    Style::new().fg_color(Some(AnsiColor::BrightBlack.into()))
}

/// One flat colour, for the three things that stand out.
fn colour(colour: AnsiColor) -> Style {
    Style::new().fg_color(Some(colour.into()))
}

#[cfg(test)]
#[path = "setup_style_tests.rs"]
mod tests;
