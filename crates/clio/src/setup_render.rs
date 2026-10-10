// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for asking a question through an in-memory reader
//! and writing the answer to an in-memory writer, which is how the wizard's
//! questions are asked when no terminal is attached.
//!
//! ## Owns
//! - `ask`, `ask_choice`, `ask_checked`, `ask_masked`
//! - The numbered-menu layout, and the refusal of an answer outside the options
//!
//! ## Does not own
//! - The terminal prompts the operator actually sees (see `setup_prompt`)
//! - Which questions exist and in what order (see `setup_steps`)
//! - What a valid answer is (see `setup_validate` and `setup_db_options`)
//! - The masked reader itself (see `setup_test_seam`)
//!
//! ## Boundary
//!
//! Changes to how a question is written for a reader and a writer should
//! normally require changes here. Changes to how a question looks on a terminal,
//! or to which questions are asked, should not require changes here.
//!
//! Keep this module focused on writing one question and reading one answer.
//!
//! # Why there is no `b` and no `q`
//!
//! The terminal prompts read raw keystrokes, where a typed `b` is consumed by
//! the prompt library and reaches no code at all. Carrying the keys here as well
//! would mean two sets of dialog keys for the same question, one of which
//! silently does nothing on a terminal. The ways out are `Esc` and `Ctrl-C`,
//! and a menu's way back is an option in its own list.
//!
//! # Why the styled fragments still matter here
//!
//! Section headings, the preview, and the summary write through
//! [`crate::setup_style`] rather than through a prompt, and `Palette::paint`
//! returns its input untouched when colour is off. A test that reads this
//! module's writer with colour off therefore sees plain text, which is what
//! makes byte-exact assertions on it possible at all.

use crate::cli_style::Palette;
use crate::setup_cli::CliError;
use crate::setup_prompt::{self, Choice};
use crate::setup_style::{self, error};
use std::io::{BufRead, Write};

/// Why a group of questions stopped.
///
/// A distinct type rather than a marker string, because the step loop has to
/// tell "go back" from "this answer was refused" and from "the operator quit".
/// A message it has to string-match is a message that breaks when the wording
/// improves.
///
/// `Usage` is separate from `Refused` because the two leave the process with
/// different exit codes and mean different things to a script: a usage stop is
/// the command line being wrong, which the operator fixes by retyping the
/// command, while a refusal is a run that could not finish. Conflating them
/// makes a missing flag look like a broken install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Signal {
    /// The operator went back to the previous question.
    Back,
    /// The operator left: `Esc` at the first question, or at the apply gate.
    Quit,
    /// The command line or an answer is wrong: a missing flag without a
    /// terminal, a value outside what the driver accepts, input that ended.
    Usage(String),
    /// A probe failed, or a value no rule accepts, and nothing was written.
    Refused(String),
}

impl Signal {
    /// The message, or an empty string for the two dialog signals.
    #[must_use]
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::Back | Self::Quit => "",
            Self::Usage(reason) | Self::Refused(reason) => reason,
        }
    }

    /// The usage stop, for the places that mean "the command line is wrong".
    ///
    /// One constructor so the two are never mixed up by a caller that has to
    /// remember which is which.
    #[must_use]
    pub(crate) fn usage(reason: impl Into<String>) -> Self {
        Self::Usage(reason.into())
    }
}

/// The line a question prints when it needs to say how to leave it.
pub(crate) const BACK_HINT: &str = "Esc goes back · Enter keeps going";

/// What one prompt produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reply {
    /// An answer the operator typed, with surrounding space removed.
    Value(String),
    /// The operator asked to go back: a menu's `Go back` option, or `Esc`.
    Back,
}

/// One free-text question.
///
/// Empty input takes `default`. No key means anything on this question, because
/// a free-text field has no room for a reserved word without swallowing a
/// legitimate one-character answer. A terminal question gets its way back from
/// `Esc`, which the library delivers; a reader cannot, so a typed field has
/// none.
///
/// # Errors
/// The stream could not be written or read, or input ended with no default to
/// fall back on.
pub(crate) fn ask(
    out: &mut dyn Write,
    reader: &mut dyn BufRead,
    palette: Palette,
    label: &str,
    default: Option<&str>,
) -> Result<Reply, CliError> {
    write_prompt(out, palette, label, default)?;
    let line = read_line(reader, label)?;
    resolve(line, default, label).map(Reply::Value)
}

/// A numbered question, one option per line, accepting the number or the name.
///
/// The way out is appended here rather than at each call site, because "there is
/// always a way out" is a property of the prompt and not of any one question.
/// It is appended last so the options that answer the question keep the numbers
/// they had.
///
/// An answer that matches neither is refused with the same choices, so a typo
/// re-asks rather than silently picking one.
///
/// # Errors
/// The stream could not be written or read, or input ended with no default.
pub(crate) fn ask_choice(
    out: &mut dyn Write,
    reader: &mut dyn BufRead,
    palette: Palette,
    label: &str,
    options: &[Choice],
    default: &str,
    leaves: bool,
) -> Result<Reply, CliError> {
    let options = setup_prompt::with_back(options, leaves);
    let mut printed = false;
    loop {
        if !printed {
            printed = true;
            let _ = writeln!(out, "\n  {}", setup_style::section(palette, label));
            for (index, option) in options.iter().enumerate() {
                let _ = writeln!(out, "    [{}] {}", index + 1, option.label);
                let _ = writeln!(
                    out,
                    "        {}",
                    setup_style::detail(palette, &option.help)
                );
            }
            let _ = writeln!(
                out,
                "  {} {}:",
                setup_style::detail(palette, "Enter a number, or press Enter for"),
                setup_style::hint(palette, default)
            );
            let _ = out.flush();
        }
        // The default goes to `resolve`, which takes it on an empty line. Reading
        // the line here rather than through `ask` keeps the label from being
        // printed a second time, because the options are now their own block.
        let line = read_line(reader, label)?;
        let typed = resolve(line, Some(default), label)?;
        if let Some(hit) = match_choice(&typed, &options, default) {
            return if hit == setup_prompt::BACK_VALUE {
                Ok(Reply::Back)
            } else {
                Ok(Reply::Value(hit))
            };
        }
        let _ = writeln!(out, "{}", error(palette, &no_such_option(&typed, &options)));
    }
}

/// A free-text question re-asked until `check` accepts it.
///
/// A refused value is printed and the same question is asked again. The value
/// is never written on a failure, which is the rule that makes this a loop
/// rather than a single call.
///
/// # Errors
/// The stream could not be written or read, or the operator pressed `b` or `q`.
pub(crate) fn ask_checked<C>(
    out: &mut dyn Write,
    reader: &mut dyn BufRead,
    palette: Palette,
    label: &str,
    default: Option<&str>,
    mut check: C,
) -> Result<String, Signal>
where
    C: FnMut(&str) -> Result<(), String>,
{
    loop {
        match ask(out, reader, palette, label, default).map_err(io_signal)? {
            Reply::Back => return Err(Signal::Back),
            Reply::Value(value) => match check(&value) {
                Ok(()) => return Ok(value),
                Err(reason) => {
                    let _ = writeln!(out, "  {}", error(palette, &reason));
                }
            },
        }
    }
}

/// A question whose answer is a secret and must not be echoed.
///
/// The prompt is written to `out` and the read comes from the seam, so a test
/// scripts a value and never opens a terminal. Nothing here formats the value
/// and nothing here writes it, which is the whole point: the typed value cannot
/// appear in the writer's bytes.
///
/// # Errors
/// The seam refused.
pub(crate) fn ask_masked(
    out: &mut dyn Write,
    label: &str,
    read: &mut dyn FnMut(&mut dyn Write, &str) -> Result<String, String>,
) -> Result<String, Signal> {
    let _ = write!(out, "{label}: ");
    let _ = out.flush();
    let value = read(out, label).map_err(|reason| Signal::Refused(format!("{label}: {reason}")))?;
    let _ = writeln!(out);
    Ok(value)
}

/// A prompt that could not be written or read, as a signal.
///
/// A broken stream is not a back and not a quit: it is a refusal the operator
/// cannot answer, so it says so.
fn io_signal(error: CliError) -> Signal {
    if error.code == 2 {
        Signal::Usage(error.message)
    } else {
        Signal::Refused(error.message)
    }
}

/// Write the prompt and flush it, so a slow answer shows the question first.
///
/// No newline: the operator's answer belongs on the same line as the question,
/// which is what every transcript in the plan shows and what a screen reader
/// needs in order to read the prompt and the answer as one question.
fn write_prompt(
    out: &mut dyn Write,
    palette: Palette,
    label: &str,
    default: Option<&str>,
) -> Result<(), CliError> {
    let written = match default {
        Some(default) => write!(out, "{label} ({})", setup_style::hint(palette, default)),
        None => write!(out, "{label}"),
    };
    let _ = written.and_then(|()| write!(out, ": "));
    out.flush()
        .map_err(|e| CliError::failed(format!("prompt failed: {e}")))
}

/// Read one line, refusing an empty read with the flag guidance.
///
/// A closed reader is not a back or a quit: it is a run with no more input,
/// which is a usage error naming the equivalent flag.
fn read_line(reader: &mut dyn BufRead, label: &str) -> Result<String, CliError> {
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|e| CliError::failed(format!("prompt failed: {e}")))?;
    if read == 0 {
        return Err(CliError::usage(format!(
            "no input for `{label}`; pass the equivalent flag"
        )));
    }
    Ok(line.trim().to_owned())
}

/// Turn a typed line into a value, taking the default on an empty line.
fn resolve(line: String, default: Option<&str>, label: &str) -> Result<String, CliError> {
    if !line.is_empty() {
        return Ok(line);
    }
    default
        .map(str::to_owned)
        .ok_or_else(|| CliError::usage(format!("`{label}` is required")))
}

/// The option a number or a name selects, or the default on an empty line.
fn match_choice(typed: &str, options: &[Choice], default: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Some(default.to_owned());
    }
    if let Ok(index) = typed.parse::<usize>()
        && let Some(option) = options.get(index.checked_sub(1)?)
    {
        return Some(option.value.to_owned());
    }
    options
        .iter()
        .find(|option| option.value.eq_ignore_ascii_case(typed))
        .map(|option| option.value.to_owned())
}

/// Why a choice answer was not one of the options.
fn no_such_option(typed: &str, options: &[Choice]) -> String {
    let names: Vec<String> = options
        .iter()
        .enumerate()
        .map(|(index, option)| format!("{} {}", index + 1, option.value))
        .collect();
    format!("`{typed}` is not one of: {}", names.join(", "))
}

#[cfg(test)]
#[path = "setup_render_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "setup_render_mask_tests.rs"]
mod setup_render_mask_tests;
