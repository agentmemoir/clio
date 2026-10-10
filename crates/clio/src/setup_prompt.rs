// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for asking the operator a question on a real
//! terminal, and for the one rule every question shares: there is always a way
//! back.
//!
//! ## Owns
//! - The terminal prompts, drawn by `cliclack`: a menu, a text field, a secret,
//!   and a yes-or-no
//! - [`Choice`], the three parts of a menu option, and the `Go back` option every
//!   menu carries
//! - Mapping a cancel from the library onto the wizard's own `Back`
//!
//! ## Does not own
//! - Which questions exist and in what order (see `setup_steps`)
//! - The plain-text renderer the tests and terminal-less runs use (see
//!   `setup_render`)
//! - What a valid answer is (see `setup_validate`)
//!
//! ## Boundary
//!
//! Changes to how a question looks on a terminal, or to the back rule, should
//! normally require changes here. Changes to what is asked should not.
//!
//! Keep this module focused on one question and one answer.
//!
//! # Why every menu carries a `Go back` item
//!
//! The library reads raw keystrokes, so a menu is answered with the arrow keys
//! and Enter. It offers no hook for an extra key, which means a `b` keypress
//! would be swallowed by the library and reach nothing. An item in the list is
//! the only way to offer the way back that the operator can actually see, and
//! putting it last means the options that answer the question stay where they
//! were.
//!
//! # Why the hint is printed on its own line
//!
//! The library prints no key hints of its own, and it reprints the prompt text
//! after the answer is chosen. A hint written into the prompt would therefore
//! be frozen into the transcript of every answer, so the hint is printed above
//! the prompt instead, where the answer leaves it behind.
//!
//! # Why a cancel means `Back` and not `Quit`
//!
//! The library reports one signal for both Esc and Ctrl-C, and cannot tell them
//! apart. It is mapped to `Back`, which the step loop already turns into a
//! clean exit at the first question. The consequence is that Ctrl-C walks back
//! one question rather than killing the run outright, and the operator reaches
//! the exit by pressing it again.

use crate::setup_cli::CliError as PromptError;
use std::cell::RefCell;
use std::io::{self, IsTerminal, Write};

/// The value the `Go back` option carries. Chosen so it cannot collide with a
/// real answer: no question's answer is a bare word in double dashes.
pub(crate) const BACK_VALUE: &str = "--go-back";

/// What the operator reads on the `Go back` option.
pub(crate) const BACK_LABEL: &str = "Go back";

/// The explanation shown while `Go back` is highlighted.
pub(crate) const BACK_HINT: &str = "return to the previous question";

/// What the option reads at the first question, where there is nothing to return
/// to and the only thing it can do is leave.
pub(crate) const QUIT_LABEL: &str = "Quit";

/// The explanation shown while `Quit` is highlighted.
pub(crate) const QUIT_HINT: &str = "leave Clio without writing anything";

/// The line printed above every prompt, naming the keys that work.
///
/// The keys differ between a menu and a field, so each prompt passes its own.
pub(crate) const MENU_KEYS: &str = "↑ ↓ move · Enter take · Esc go back";
/// The keys for a question the operator types into.
pub(crate) const FIELD_KEYS: &str = "Enter take · Esc go back";

/// One entry in a menu.
///
/// The description is a separate line rather than more text on the label. Three
/// options rendered inline become one unreadable line, so the label names the
/// choice and the description says what it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    /// The word an answer may spell, and the value the answer resolves to.
    pub value: &'static str,
    /// What the operator reads on the option's own line.
    pub label: String,
    /// One line saying what choosing this means.
    pub help: String,
}

impl Choice {
    /// One option, from its three parts.
    pub(crate) fn new(
        value: &'static str,
        label: impl Into<String>,
        help: impl Into<String>,
    ) -> Self {
        Self {
            value,
            label: label.into(),
            help: help.into(),
        }
    }

    /// The same option with a note appended to its label, as in
    /// `Clio Manages Postgres in Docker (Docker is running)`.
    ///
    /// Taken as a builder rather than a second constructor so a caller that knows
    /// the state at runtime cannot forget to build the label itself and leave the
    /// option silent about whether it will work.
    pub(crate) fn with_note(mut self, note: &str) -> Self {
        self.label = format!("{} ({note})", self.label);
        self
    }
}

/// Every option of a question, with the way out appended last.
///
/// Built here rather than at each call site so that "there is always a way out"
/// is a property of the prompt layer and cannot be forgotten by a caller that
/// adds a new question.
///
/// `leaves` says what that last option does. At the first question of a run
/// there is nothing behind it, so the option leaves the wizard and says so
/// rather than promising a return that cannot happen. Everywhere else it goes
/// back to the previous question. The value is the same either way, so the
/// caller handles one reply.
pub(crate) fn with_back(options: &[Choice], leaves: bool) -> Vec<Choice> {
    let mut all = options.to_vec();
    all.push(if leaves {
        Choice::new(BACK_VALUE, QUIT_LABEL, QUIT_HINT)
    } else {
        Choice::new(BACK_VALUE, BACK_LABEL, BACK_HINT)
    });
    all
}

/// Whether a terminal can be asked anything.
///
/// Both ends must be a terminal. The library draws to stderr and reads keys from
/// the terminal, so a run with either end redirected gets no prompts at all, and
/// the wizard's flag-driven path is the only thing left. This is the same
/// condition the wizard already uses to decide whether to prompt, so the check
/// lives beside the prompts it guards.
#[must_use]
pub(crate) fn terminal_available() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

/// Print the key hint above a prompt, on the writer the wizard already owns.
///
/// The library writes to stderr while the wizard writes to the writer it was
/// given, so the caller flushes before the prompt starts. Without that flush the
/// two streams can interleave and the hint arrives after the question.
pub(crate) fn hint(out: &mut dyn Write, keys: &str) {
    let _ = writeln!(out, "  \x1b[2m{keys}\x1b[0m");
    let _ = out.flush();
}

/// A chosen menu value as the wizard's own answer: the way out becomes `None`.
fn answer_value(answer: String) -> Option<String> {
    if answer == BACK_VALUE {
        None
    } else {
        Some(answer)
    }
}

/// Ask a menu question and return the chosen value, or `None` to go back.
///
/// # Errors
/// The terminal could not be read or drawn.
pub(crate) fn menu(
    out: &mut dyn Write,
    label: &str,
    options: &[Choice],
    default: &str,
) -> Result<Option<String>, PromptError> {
    hint(out, MENU_KEYS);
    let mut builder = cliclack::select(label);
    for option in options {
        builder = builder.item(
            option.value.to_owned(),
            option.label.clone(),
            option.help.clone(),
        );
    }
    let answer: io::Result<String> = builder.initial_value(default.to_owned()).interact();
    answer.map_err(terminal_error).map(answer_value)
}

/// Ask for a line of text, or `None` to go back.
///
/// # Errors
/// The terminal could not be read or drawn.
pub(crate) fn text(
    out: &mut dyn Write,
    label: &str,
    default: Option<&str>,
) -> Result<Option<String>, PromptError> {
    hint(out, FIELD_KEYS);
    let mut builder = cliclack::input(label);
    if let Some(default) = default {
        builder = builder.default_input(default);
    }
    let answer: io::Result<String> = builder.interact();
    answer.map(Some).map_err(terminal_error)
}

/// Ask for a line of text that has to satisfy a rule, or `None` to go back.
///
/// The rule is handed to the prompt rather than checked after it, so a refused
/// value shows its reason with the question still open. Checking afterwards
/// would close the question, print the reason, and open it again, which reads as
/// three questions where there was one.
///
/// The rule has to own everything it uses: the prompt holds it for as long as it
/// is on screen, so it cannot borrow from the caller.
///
/// # Errors
/// The terminal could not be read or drawn.
pub(crate) fn checked_text(
    out: &mut dyn Write,
    label: &str,
    default: Option<&str>,
    check: impl FnMut(&str) -> Result<(), String> + 'static,
) -> Result<Option<String>, PromptError> {
    hint(out, FIELD_KEYS);
    let mut builder = cliclack::input(label);
    if let Some(default) = default {
        builder = builder.default_input(default);
    }
    // The wizard's rules are `FnMut` because a field may check a value more than
    // once, while the prompt wants a rule it can call without taking ownership of
    // it. The cell is what lets one be the other: the closure borrows it, and
    // borrowing is all it does.
    let check = RefCell::new(check);
    let answer: io::Result<String> = builder
        .validate(move |value: &String| (check.borrow_mut())(value))
        .interact();
    answer.map(Some).map_err(terminal_error)
}

/// Ask for a secret. It is drawn masked and never printed again.
///
/// # Errors
/// The terminal could not be read or drawn.
pub(crate) fn secret(out: &mut dyn Write, label: &str) -> Result<Option<String>, PromptError> {
    hint(out, FIELD_KEYS);
    let answer: io::Result<String> = cliclack::password(label).mask('▪').interact();
    answer.map(Some).map_err(terminal_error)
}

/// Ask a yes-or-no question, or `None` to go back.
///
/// The highlight starts on `No`, because this question is the last one before
/// anything is written and a stray Enter should not do that.
///
/// # Errors
/// The terminal could not be read or drawn.
pub(crate) fn confirm(out: &mut dyn Write, label: &str) -> Result<Option<bool>, PromptError> {
    hint(out, FIELD_KEYS);
    let answer: io::Result<bool> = cliclack::confirm(label).initial_value(false).interact();
    answer.map(Some).map_err(terminal_error)
}

/// A cancel, a dead terminal, or a read failure, as the wizard's own error.
///
/// The library cannot tell a cancel from a closed terminal by shape alone, so
/// both land here as a clean exit, and the caller turns a clean exit into going
/// back rather than into a refusal the operator can do nothing about.
// The error arrives by value through `Result::map_err`, which hands the closure
// ownership of it. Borrowing here instead would mean every one of the five call
// sites had to bind the error first, for no gain: the value is formatted once and
// dropped.
#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err hands the closure ownership; borrowing would add a binding per call site"
)]
fn terminal_error(error: io::Error) -> PromptError {
    match error.kind() {
        io::ErrorKind::Interrupted | io::ErrorKind::NotConnected => PromptError::clean(),
        _ => PromptError::failed(format!("the question could not be asked: {error}")),
    }
}

#[cfg(test)]
#[path = "setup_prompt_tests.rs"]
mod setup_prompt_tests;
