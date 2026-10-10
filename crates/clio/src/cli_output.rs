// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared CLI output contract: output-mode
//! selection, JSON/text emission, terminal/color detection, and error rendering.
//!
//! ## Owns
//! - `OutputMode` selection (explicit `--output` beats TTY detection)
//! - JSON (one line, stdout data) and human-text emission
//! - TTY and `NO_COLOR` detection and the colored error line
//! - The `{ok:false, code, message, hint?}` envelope for non-TTY/JSON errors
//!
//! ## Does not own
//! - The error type itself (see `cli_error`)
//! - Per-command text tables (see `cli_read`)
//! - Argument parsing (see `cli_args`)
//!
//! ## Boundary
//!
//! Changes to the output/color contract should normally require changes here.
//! Changes to a command's data shape should not require changes here.
//!
//! Keep this module focused on the shared CLI output contract.

use crate::cli_error::CliError;
use serde_json::Value;
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

/// How a command renders its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputMode {
    /// One compact JSON line (the piped default).
    Json,
    /// Human-readable text (the TTY default).
    Text,
}

/// Resolve the output mode: an explicit `--output` always wins over TTY detection.
///
/// The piped default is JSON so `clio recall q | jq` works blind; on a TTY the
/// default is human text.
pub(crate) fn resolve_output(explicit: Option<&str>, is_tty: bool) -> Result<OutputMode, CliError> {
    match explicit {
        Some("json") => Ok(OutputMode::Json),
        Some("text") => Ok(OutputMode::Text),
        Some(other) => Err(CliError::usage_hint(
            format!("--output must be `json` or `text`, got `{other}`"),
            "try `--output json` or `--output text`",
        )),
        None => Ok(if is_tty {
            OutputMode::Text
        } else {
            OutputMode::Json
        }),
    }
}

/// True when stdout is attached to a terminal.
pub(crate) fn stdout_is_tty() -> bool {
    io::stdout().is_terminal()
}

/// Resolve the output mode from raw, possibly unparseable arguments.
///
/// Error paths cannot trust a full parse, but an explicit `--output json|text`
/// that is present must still beat TTY detection; a missing, unknown, or
/// value-less `--output` falls back to the terminal default.
pub(crate) fn mode_from_raw(args: &[String], is_tty: bool) -> OutputMode {
    match explicit_output(args) {
        Some("json") => OutputMode::Json,
        Some("text") => OutputMode::Text,
        _ => {
            if is_tty {
                OutputMode::Text
            } else {
                OutputMode::Json
            }
        }
    }
}

/// The value of the last explicit `--output` / `--output=…` / `-o` / `-o=…`
/// token in `args`.
///
/// A bare `--output` (or `-o`) followed by another flag (or nothing) has no
/// value and is ignored, mirroring the parser's fail-closed missing-value rule.
fn explicit_output(args: &[String]) -> Option<&str> {
    let mut found = None;
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        index += 1;
        if token == "--output" || token == "-o" {
            if let Some(value) = args.get(index).filter(|value| !value.starts_with("--")) {
                found = Some(value.as_str());
                index += 1;
            }
        } else if let Some(value) = token
            .strip_prefix("--output=")
            .or_else(|| token.strip_prefix("-o="))
        {
            found = Some(value);
        }
    }
    found
}

/// True when `NO_COLOR` is set (any value, per the informal `NO_COLOR` standard).
pub(crate) fn no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

/// True when `CLICOLOR_FORCE` is set, asking for color even off a terminal.
///
/// The counterpart to `NO_COLOR` for the piped case: without it, output going to
/// a pager or a log collector has no way to ask for color.
pub(crate) fn color_forced() -> bool {
    std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| v != "0")
}

/// Color is used on a terminal unless `NO_COLOR` is set, and `CLICOLOR_FORCE`
/// overrides both directions: it forces color on, and `NO_COLOR` still wins when
/// both are set, because an explicit "no color" is the stronger signal.
pub(crate) fn supports_color(is_tty: bool, no_color: bool) -> bool {
    if no_color {
        return false;
    }
    is_tty || color_forced()
}

/// Write one compact JSON value followed by a newline.
pub(crate) fn write_json<W: Write>(w: &mut W, value: &Value) -> io::Result<()> {
    writeln!(
        w,
        "{}",
        serde_json::to_string(value).expect("serialize cli json")
    )
}

/// Write a human-text block followed by a newline.
pub(crate) fn write_text<W: Write>(w: &mut W, text: &str) -> io::Result<()> {
    writeln!(w, "{text}")
}

/// Render one failure under the selected mode; `color` paints the text line red.
pub(crate) fn write_error<W: Write>(
    w: &mut W,
    error: &CliError,
    mode: OutputMode,
    color: bool,
) -> io::Result<()> {
    match mode {
        OutputMode::Json => write_json(w, &error.to_envelope()),
        OutputMode::Text => {
            let text = error.to_human();
            if color {
                writeln!(w, "\u{1b}[31m{text}\u{1b}[0m")
            } else {
                write_text(w, &text)
            }
        }
    }
}

/// Emit a failure to stderr under the resolved mode and color state.
///
/// Used where no command parsed the arguments (for example an unknown top-level
/// command): the caller resolves the mode from the raw tokens so an explicit
/// `--output` still wins, and `is_tty`/`no_color` decide only the color.
pub(crate) fn report_error(
    error: &CliError,
    mode: OutputMode,
    is_tty: bool,
    no_color: bool,
) -> ExitCode {
    let mut stderr = io::stderr();
    // A failed stderr write is not actionable; the exit code still reports the failure.
    let _ = write_error(&mut stderr, error, mode, supports_color(is_tty, no_color));
    error.exit_code()
}

#[cfg(test)]
#[path = "cli_output_tests.rs"]
mod tests;
