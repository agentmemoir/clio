// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for asking the operator one question and turning
//! the answer into the wizard's own vocabulary, whichever renderer draws it.
//!
//! ## Owns
//! - Every question the wizard asks: a menu, a text field, a secret, a yes-or-no
//! - The switch between the terminal prompt and the plain one
//! - Mapping a prompt failure, a cancel, and a refusal onto a `Signal`
//!
//! ## Does not own
//! - How a question looks on a terminal (see `setup_prompt`)
//! - How a question looks without one (see `setup_render`)
//! - Which questions exist and in what order (see `setup_steps`)
//! - The run's flags, streams, and probed facts (see `setup_cli_flows`)
//!
//! ## Boundary
//!
//! Changes to how a question is asked, or to how its answer is reported, should
//! normally require changes here. Changes to what a question says, or to which
//! questions there are, should not.
//!
//! Keep this module focused on one question and one answer.
//!
//! # Why these methods live apart from the state they read
//!
//! The state is one struct and the questions are one responsibility, and the two
//! together outgrew a single file. Splitting them keeps each file describable in
//! one sentence, which is the only reason to split anything.
//!
//! # Why there is one switch and not a check per question
//!
//! A terminal can be attached to one end of a run and not the other. Deciding per
//! question would let a single run draw half its questions one way and half the
//! other, which is worse than either renderer alone. So the fact is captured once
//! and every question reads it.

use crate::setup_cli::CliError;
use crate::setup_cli_flows::Ctx;
use crate::setup_prompt::Choice;
use crate::setup_render::{self, Reply, Signal};
use crate::setup_secrets::production_reader;
use std::io::Write;

impl Ctx<'_> {
    /// A free-text question.
    ///
    /// # Errors
    /// The stream failed, or the terminal could not be drawn.
    pub(crate) fn ask(&mut self, label: &str, default: Option<&str>) -> Result<Reply, Signal> {
        let answered = if self.terminal {
            self.ask_on_terminal(|out| crate::setup_prompt::text(out, label, default))
                .map(|answer| match answer {
                    Some(value) => Reply::Value(value),
                    None => Reply::Back,
                })?
        } else {
            setup_render::ask(self.out, self.reader, self.palette, label, default)
                .map_err(io_signal)?
        };
        Ok(self.note_answered(answered))
    }

    /// A yes-or-no question with `No` highlighted, or `None` to go back.
    ///
    /// The highlight is on `No` because this question is the last one before
    /// anything is written, and a stray `Enter` should not do that.
    ///
    /// # Errors
    /// The stream failed, or the terminal could not be drawn.
    pub(crate) fn ask_yes_no(&mut self, label: &str) -> Result<Option<bool>, Signal> {
        if self.terminal {
            return self.ask_on_terminal(|out| crate::setup_prompt::confirm(out, label));
        }
        // A reader has no way to show a two-way choice but a line of text, so the
        // plain renderer asks the same question and reads `y` or `n`.
        let answer = setup_render::ask(self.out, self.reader, self.palette, label, Some("n"))
            .map_err(io_signal)?;
        match answer {
            Reply::Back => Ok(None),
            // Case-insensitively, and only for the two words that mean yes.
            // Anything else is `no`, which is the safe direction for the one
            // question that writes: an operator who typed `Yes` and was answered
            // `no` would be told their settings were saved and they were not.
            Reply::Value(value) => {
                let value = value.trim();
                Ok(Some(matches!(
                    value.to_ascii_lowercase().as_str(),
                    "y" | "yes"
                )))
            }
        }
    }

    /// Run a terminal prompt, turning a cancel into `None`.
    ///
    /// The prompt library reports one signal for `Esc` and `Ctrl-C` and cannot
    /// tell them apart, so both come back as `None`. The step loop already turns
    /// a back at the first question into a clean exit, which is what makes one
    /// signal enough for both keys.
    ///
    /// # Errors
    /// The terminal could not be drawn, for a reason other than a cancel.
    fn ask_on_terminal<T>(
        &mut self,
        ask: impl FnOnce(&mut dyn Write) -> Result<Option<T>, CliError>,
    ) -> Result<Option<T>, Signal> {
        // The prompt draws to stderr while the wizard writes to `out`, so the
        // hint printed just above the question has to be on the screen first.
        let _ = self.out.flush();
        match ask(self.out) {
            Ok(answer) => Ok(answer),
            Err(error) if error.code == 0 => Ok(None),
            Err(error) => Err(io_signal(error)),
        }
    }

    /// A menu question, accepting the number or the option's name.
    ///
    /// The way out is added by the prompt layer, so every menu a caller asks for
    /// carries one without the caller having to remember.
    ///
    /// # Errors
    /// The stream failed, or the terminal could not be drawn.
    pub(crate) fn ask_choice(
        &mut self,
        label: &str,
        options: &[Choice],
        default: &str,
    ) -> Result<Reply, Signal> {
        // Whether the last option leaves the run is decided here rather than by the
        // step loop, because a step can ask many questions and only the prompt
        // knows one has been answered. The first question of a run offers `Quit`;
        // every question after it offers `Go back`, however deep inside a step it
        // is.
        let leaves = self.back_leaves_run;
        let answered = if self.terminal {
            let options = crate::setup_prompt::with_back(options, leaves);
            self.ask_on_terminal(|out| crate::setup_prompt::menu(out, label, &options, default))
                .map(|answer| match answer {
                    Some(value) => Reply::Value(value),
                    None => Reply::Back,
                })?
        } else {
            setup_render::ask_choice(
                self.out,
                self.reader,
                self.palette,
                label,
                options,
                default,
                leaves,
            )
            .map_err(io_signal)?
        };
        Ok(self.note_answered(answered))
    }

    /// Record that a question produced a value, and hand the reply back.
    ///
    /// One place, because a step can ask several questions and each of the three
    /// question methods has to clear the same flag. Clearing it on any answered
    /// question rather than only on a menu is what makes the flag mean what it
    /// says: there is something behind the operator once anything has been
    /// answered, whichever kind of question answered it.
    fn note_answered(&mut self, answered: Reply) -> Reply {
        if matches!(answered, Reply::Value(_)) {
            self.back_leaves_run = false;
        }
        answered
    }

    /// A menu question, kept as a named entry point so the database choice and
    /// the hand-over choice cannot drift into different rendering.
    ///
    /// # Errors
    /// The stream failed, or the terminal could not be drawn.
    pub(crate) fn ask_choice_aliased(
        &mut self,
        label: &str,
        options: &[Choice],
        default: &str,
    ) -> Result<Reply, Signal> {
        self.ask_choice(label, options, default)
    }

    /// A free-text question re-asked until `check` accepts it.
    ///
    /// # Errors
    /// The stream failed, or the operator pressed a dialog key.
    pub(crate) fn ask_checked<C>(
        &mut self,
        label: &str,
        default: Option<&str>,
        check: C,
    ) -> Result<String, Signal>
    where
        C: FnMut(&str) -> Result<(), String> + 'static,
    {
        let answered = if self.terminal {
            self.ask_on_terminal(|out| {
                crate::setup_prompt::checked_text(out, label, default, check)
            })?
            .ok_or(Signal::Back)?
        } else {
            setup_render::ask_checked(self.out, self.reader, self.palette, label, default, check)?
        };
        self.back_leaves_run = false;
        Ok(answered)
    }

    /// A secret, read through the masked reader so it is never echoed.
    ///
    /// The reader is moved out for the call and moved back afterwards, because
    /// `ask_masked` needs a `&mut dyn FnMut` and `self` is already borrowed by
    /// the other two fields.
    ///
    /// On a terminal the prompt library does its own masking, so the masked
    /// reader is only reached when there is no terminal to mask against.
    ///
    /// # Errors
    /// The seam refused, or the operator asked to go back.
    pub(crate) fn ask_password(&mut self, label: &str) -> Result<String, Signal> {
        if self.terminal {
            return self
                .ask_on_terminal(|out| crate::setup_prompt::secret(out, label))?
                .ok_or(Signal::Back);
        }
        let mut secrets = std::mem::replace(&mut self.secrets, production_reader());
        let outcome = setup_render::ask_masked(self.out, label, &mut *secrets);
        self.secrets = secrets;
        outcome
    }
}

/// A prompt failure as a signal, by exit code.
///
/// A clean exit is not one: that is a cancel, and the caller turns it into going
/// back rather than into a refusal the operator can do nothing about.
fn io_signal(error: CliError) -> Signal {
    if error.code == 2 {
        Signal::Usage(error.message)
    } else {
        Signal::Refused(error.message)
    }
}

#[cfg(test)]
#[path = "setup_ask_tests.rs"]
mod setup_ask_tests;
