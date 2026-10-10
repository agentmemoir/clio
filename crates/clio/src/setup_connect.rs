// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for running one real probe and reporting what it
//! said, in the colours the rest of the wizard uses.
//!
//! ## Owns
//! - The `Testing <what> ...` line and the `ok` / `failed` that follows it
//! - Turning a probe's refusal into an indented line the operator can read
//! - Passing the probe's own message through untouched
//! - The one sentence a waived probe prints, shared by all three call sites
//!
//! ## Does not own
//! - Building the probes (see `setup_test_seam`)
//! - What a failure means for which field (see `setup_validate::Blame`)
//! - Deciding whether a failed test may still be saved (stage 5 owns that)
//!
//! ## Boundary
//!
//! Changes to how a probe result is shown should normally require changes
//! here. Changes to what a probe does, or to the colours, should not require
//! changes here.
//!
//! Keep this module focused on reporting one probe's outcome.
//!
//! # Why the message is passed through
//!
//! The probe is the store's own connection code and the index's own transport,
//! so its message is the one the operator would get from any other Clio
//! command. Summarising it here would hide the only part that is actionable,
//! which is exactly what the `db error` placeholder problem is about.
//!
//! Passing it through is not the same as printing it verbatim: the credentials
//! in a connection string are masked first, which is what the rest of Clio does
//! with the same string. An address is worth showing; the password inside it is
//! not, and a terminal scrollback is not a secret store.
//!
//! # Why the attempt and the outcome share a line
//!
//! The attempt is written and flushed with no newline, so a probe that takes
//! several seconds shows something while it runs, and the `ok` or `failed`
//! lands on the same line when it finishes. A refusal then goes on the line
//! below, indented, because it is a sentence rather than a verdict.

use crate::cli_style::Palette;
use anstyle::{AnsiColor, Style};
use std::io::Write;

/// Run one probe, reporting the attempt and the outcome.
///
/// The probe is injected so this function can be tested without a server and
/// so stage 6 can hand it the real ones without this module knowing how they
/// are built.
///
/// Returns whatever the probe returned on success, so the caller can show the
/// facts it learned — the server version, whether the vector extension is
/// there — beside the `ok`.
///
/// # Errors
/// Whatever the probe refused with, with any credentials masked. The refusal has
/// already been printed by the time this returns `Err`.
///
/// The writer is a trait object, as `setup_wizard::execute` takes it: this is
/// a rendering entry point that stage 5 calls with the same stream everything
/// else writes to, not a function each caller specialises.
pub(crate) fn run(
    writer: &mut dyn Write,
    palette: Palette,
    what: &str,
    probe: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    let _ = write!(writer, "Testing {what} ...");
    let _ = writer.flush();
    // A probe is handed the connection string, and the most useful thing it can
    // say is which address it tried — which on the typed route carries the
    // password the operator just typed. Redacting here rather than at each print
    // site is what makes that safe: the reason is displayed by this function, and
    // then again by the retry message, the blame line, and the final refusal, so
    // masking one of those would leave the others leaking. The success detail
    // gets the same treatment because it reaches the preview the same way.
    let outcome = probe().map(|detail| clio_ops::redact_credentials(&detail));
    let outcome = outcome.map_err(|reason| clio_ops::redact_credentials(&reason));
    let _ = writeln!(writer, " {}", verdict(palette, &outcome));
    // A refusal is a sentence rather than a verdict, so it goes on its own line.
    if let Err(reason) = &outcome {
        let _ = writeln!(writer, "  {reason}");
    }
    outcome
}

/// The warning a waived probe prints, in one spelling.
///
/// Three call sites waive a probe: the two question-time ones and the post-apply
/// one. They share this sentence so an operator who waived a test reads the same
/// words wherever the waiver happened.
pub(crate) const UNTESTED_WARNING: &str =
    "Clio will not work until this is fixed. --allow-untested saved the answers anyway.";

/// The one word that closes a probe line, with the colour that goes with it.
///
/// One place decides both, so the two arms cannot drift into a green `failed`.
fn verdict(palette: Palette, outcome: &Result<String, String>) -> String {
    let (colour, word) = if outcome.is_ok() {
        (AnsiColor::Green, "ok")
    } else {
        (AnsiColor::Red, "failed")
    };
    palette.paint(Style::new().fg_color(Some(colour.into())), word)
}

#[cfg(test)]
#[path = "setup_connect_tests.rs"]
mod tests;
