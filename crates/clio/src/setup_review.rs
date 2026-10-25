// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for what happens after every question is answered:
//! drawing the preview, taking the apply/amend/quit answer, and re-drawing after
//! an amendment.
//!
//! ## Owns
//! - `review`, the loop that draws, prompts, and redraws
//! - `Verdict`, what the operator chose: apply, or quit
//! - `StoreState`, whether a database change needs the typed `switch`
//!
//! ## Does not own
//! - The questions (see `setup_steps`)
//! - The preview's shape and wording (see `setup_report`)
//! - The amend and `switch` prompts (see `setup_report_amend`)
//! - Writing the config and counting the rows in the store (stage 6)
//!
//! ## Boundary
//!
//! Changes to the order "preview, then ask, then apply or redraw" should
//! normally require changes here. Changes to what a block says, or to which
//! section `a` re-asks, should not require changes here.
//!
//! Keep this module focused on the last loop of a run.
//!
//! # Why the loop returns a verdict rather than writing
//!
//! Stage 5 asks and previews; stage 6 applies. Returning the answers and the
//! verdict keeps the ordering the plan fixes — nothing is written before the
//! preview, and nothing is written at all until the operator has agreed — without
//! this module having to know how a config is written. It also means the tests
//! here drive a real operator session and assert on what was printed, not on a
//! file.
//!
//! # Why `StoreState` is passed in rather than read
//!
//! Whether a database change is destructive depends on how many memories the
//! current store holds, and counting them is a database read. Reading it here
//! would make the preview depend on the store being reachable, which is exactly
//! the state a first run is not in. Stage 6 supplies the counts; a caller that
//! does not know passes [`StoreState::Unknown`], which asks rather than assumes.

use crate::setup_cli_flows::Ctx;
use crate::setup_preview as preview;
use crate::setup_render::Signal;
use crate::setup_report as report;
use crate::setup_report_amend::Amend;
use crate::setup_steps::{Answers, Step};
use serde_json::Value;

/// What the operator chose once the answers were complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Enter, or a non-interactive run: the answers stand and the caller applies.
    Apply,
    /// `q`: stop, write nothing.
    Quit,
}

/// What one pass through the post-preview question decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// The answers stand.
    Apply,
    /// Stop, write nothing.
    Quit,
    /// Re-ask one section and draw the preview again.
    Amend(Amend),
}

/// What is known about the store the run would leave behind.
///
/// `Empty` and `NonEmpty` are the two states that decide whether a database
/// change needs the typed `switch`; `Unreadable` closes the gate the same way
/// `NonEmpty` does. `Unknown` is the value a caller that has not counted passes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreState {
    /// The counts are not known, so no `switch` is asked for.
    ///
    /// Passed by the stage 5 test harness and by any caller that cannot count.
    #[allow(dead_code, reason = "the stage 5 harness passes it")]
    Unknown,
    /// The store holds nothing, so a change loses nothing.
    Empty,
    /// The store holds this many memories and subjects, and the new target is
    /// not the store currently configured.
    NonEmpty {
        /// How many memories the current store holds.
        memories: u64,
        /// How many subjects the current store holds.
        subjects: u64,
    },
    /// The store exists but could not be read, so its contents are unknown.
    ///
    /// An unreadable store is not a licence to skip the gate: the counts could
    /// be anything, including everything the operator cares about, so the typed
    /// confirmation is required and the reason is printed.
    Unreadable {
        /// Why the store could not be read.
        reason: String,
    },
}

impl StoreState {
    /// The sentence the `switch` prompt leads with, or `None` when there is
    /// nothing to warn about.
    #[must_use]
    pub(crate) fn consequence(&self) -> Option<String> {
        match self {
            // Counts are singular at one. A setup that reports "1 memories" reads
            // as a bug in the counter, which is exactly the doubt an operator
            // should not have about a number they are about to act on.
            Self::NonEmpty { memories, subjects } => Some(format!(
                "You have {memories} {} about {subjects} {} in the database Clio uses today.",
                if *memories == 1 { "memory" } else { "memories" },
                if *subjects == 1 {
                    "subject"
                } else {
                    "subjects"
                },
            )),
            Self::Unreadable { reason } => Some(format!(
                "Clio could not read the database it is using now ({reason}), so it cannot say how \
                 many memories are in it. They will not be visible from the new one."
            )),
            Self::Unknown | Self::Empty => None,
        }
    }
}

/// Draw the preview, take the answer, and redraw after an amendment.
///
/// The loop continues until the operator applies or quits, which is the plan's
/// shape: amending jumps straight to a section rather than walking back one
/// question at a time, and the preview is redrawn so the change is visible before
/// anything is agreed to.
///
/// `compose_dir` is the `--compose-dir` value: the root the extract model's
/// presence is checked under here, and the same root the download uses once the
/// operator has agreed. The check is a presence test, so the preview names a
/// download without performing one.
///
/// # Errors
/// The stream could not be written or read, the amend question named no
/// section, or the `switch` confirmation was not typed.
#[allow(
    clippy::needless_pass_by_value,
    reason = "the store state describes this run and the caller stops needing it"
)]
pub(crate) fn review(
    ctx: &mut Ctx<'_>,
    mut answers: Answers,
    current: &dyn Fn(&str) -> Option<Value>,
    will_back_up: Option<&str>,
    compose_dir: Option<&str>,
    store: StoreState,
) -> Result<(Answers, Verdict), Signal> {
    loop {
        let env = &|key: &str| ctx.env(key);
        // The download is announced before the operator accepts it, and the
        // presence check runs with the answers as they stand, so an amendment that
        // turns the extractor on or off changes this line with the redraw.
        let announced = crate::setup_extract_model::announcement(&answers, compose_dir, env);
        let view = preview::view(
            &answers,
            current,
            will_back_up.map(str::to_owned),
            announced.will_fetch,
            announced.will_stop,
            env,
        );
        let palette = ctx.palette();
        report::preview(ctx.writer(), palette, &view).map_err(Signal::Refused)?;
        // The consequence is drawn under the preview as well as at the typed
        // confirmation, because this is the moment the operator decides. The
        // plan requires the preview to name the counts and to say that the old
        // store is left alone and this wizard does not move data.
        if let Some(reason) = store.consequence() {
            // The label is the first line because the block arrives after the
            // `Tested` list, and a paragraph of consequence with no heading reads
            // as more test output. It names the subject rather than the severity:
            // the severity is already carried by the colour, and a heading that
            // shouted would train the operator to stop reading it.
            let notice = format!(
                "Your existing data\n  {reason}\n  {}",
                crate::setup_report_amend::not_moving_data()
            );
            let painted = crate::setup_style::warn(palette, &notice);
            writeln!(ctx.writer(), "  {painted}")
                .map_err(|e| Signal::Refused(format!("failed to write the preview: {e}")))?;
            writeln!(ctx.writer())
                .map_err(|e| Signal::Refused(format!("failed to write the preview: {e}")))?;
        }
        match ask_to_apply(ctx, &answers, &store)? {
            Next::Apply => return Ok((answers, Verdict::Apply)),
            Next::Quit => {
                crate::setup_steps::announce_quit(ctx);
                return Ok((answers, Verdict::Quit));
            }
            Next::Amend(section) => {
                let from = Step::first_of(section);
                answers.drop_from(from);
                answers = crate::setup_steps::walk(ctx, answers, from)?;
            }
        }
    }
}

/// The post-preview question: apply, or go back and change an answer.
///
/// The highlight starts on `No`, because this is the last question before
/// anything is written and a stray `Enter` should not do that. `No` leads to the
/// amend menu rather than to leaving, because the preview is still on screen and
/// the operator may well want to change one thing and come straight back. `Esc`
/// is how a run leaves with nothing written.
///
/// # Errors
/// The question could not be asked, the operator left, or a section's questions
/// were refused.
fn ask_to_apply(ctx: &mut Ctx<'_>, answers: &Answers, store: &StoreState) -> Result<Next, Signal> {
    // A non-interactive run has no one to ask, so the answers stand. That is the
    // case where the operator has already said everything there is to say, in
    // flags.
    if !ctx.interactive() {
        return Ok(Next::Apply);
    }
    match ctx.ask_yes_no("Write these settings and start the containers?")? {
        Some(true) => {
            if let Some(reason) = store.consequence() {
                crate::setup_report_amend::switch(ctx, &reason)?;
            }
            Ok(Next::Apply)
        }
        Some(false) => {
            let offered: Vec<Amend> = Amend::ALL
                .into_iter()
                .filter(|section| answered(answers, *section))
                .collect();
            crate::setup_report_amend::amend_section(ctx, &offered)
                .map(|section| section.map_or(Next::Apply, Next::Amend))
        }
        // `Esc` and `Ctrl-C` both land here, and both leave with nothing written.
        None => Ok(Next::Quit),
    }
}

/// Whether a section has an answer to re-ask.
///
/// A section the operator never configured cannot be amended, because there is
/// nothing there to change and re-asking would ask again a question they had
/// already answered by declining.
fn answered(answers: &Answers, section: Amend) -> bool {
    match section {
        Amend::Database => answers.database.is_some(),
        Amend::Embedding => answers.embed_role.is_some(),
        Amend::Reranking => answers.rerank_role.is_some(),
        Amend::Extraction => answers.extract.is_some(),
    }
}

#[cfg(test)]
#[path = "setup_review_tests.rs"]
mod tests;

/// The dead-terminal paths, in their own module.
///
/// Split out because the shared harness cannot produce a stream that fails, and
/// these three writes and reads are the only places in this file that report one.
#[cfg(test)]
#[path = "setup_review_streams_tests.rs"]
mod streams_tests;
