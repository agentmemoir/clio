// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the order `clio setup` asks its questions in,
//! for the answers collected so far, and for going back and quitting.
//!
//! ## Owns
//! - `Step`, the step list, and `plan`, which rebuilds it from the answers
//! - `Answers`, the whole collected state, and dropping the tail above a step
//! - `drive`, the loop from the first question to the last
//! - Announcing the discarded section by name, before dropping it
//!
//! ## Does not own
//! - What any question asks (see `setup_step_ask`)
//! - Rendering (see `setup_render`)
//! - The preview and the amend loop (see `setup_preview` and `setup_review`)
//!
//! ## Boundary
//!
//! Changes to the step list or to how going back behaves should normally
//! require changes here. Changes to a single question should not.
//!
//! Keep this module focused on navigation.
//!
//! # Why the tail is rebuilt rather than filtered in place
//!
//! The database choice decides whether the hand-over question exists, and each
//! service's role decides whether its detail questions exist. So the step list is
//! a function of the answers, not a constant. Rebuilding it after every answer
//! from [`Answers`] means a route cannot leave a step behind that no longer
//! applies, which is the failure that makes a wizard ask about a service the
//! operator already declined.
//!
//! # Why answers below the step are kept
//!
//! The plan is explicit: walking back three times and returning does not re-ask
//! the questions above the step. So the tail from the step being left upwards is
//! dropped and everything below survives. Nothing below is ever cleared because
//! a later step disappeared, because clearing a typed password because a later
//! step was dropped is exactly the bug that loses it.
//!
//! # Why the loop keeps a cursor rather than a shrinking list
//!
//! Going back needs the step *before* the current one, and a list that starts at
//! the current step does not contain it. An index into the rebuilt list does,
//! and it also survives the rebuild: after a `Done` the step that was answered
//! may no longer be in the list at all, which is exactly the route change the
//! rebuild exists to express.

use clio_config::KeySource;

use crate::setup_cli_flows::Ctx;
use crate::setup_render::Signal;
use crate::setup_report_amend::Amend;
use crate::setup_services::ServiceRole;
use crate::setup_step_ask::{self, Outcome};
use crate::setup_style::{going_back, hint, warn};

/// One step in the wizard.
///
/// The service role and its detail questions are separate steps rather than one
/// step with a branch inside it, because the detail step has to *disappear* when
/// the role is local. A branch inside one step could not be dropped by going
/// back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Where the encryption key is kept, asked once and only while no record of it
    /// exists.
    KeySource,
    /// Where the sync key is kept, asked only while sync is in play and no record
    /// of a choice exists.
    SyncKeySource,
    /// Where the memories live, and for an existing server how it is handed over.
    Database,
    /// Where embeddings come from.
    EmbedRole,
    /// The embedding endpoint, model, and key, only when it is hosted.
    EmbedService,
    /// Where reranking comes from.
    RerankRole,
    /// The rerank endpoint, model, and key, only when it is hosted.
    RerankService,
    /// Whether extraction runs locally, is skipped, or is hosted.
    Extract,
}

impl Step {
    /// Every step, in the order a full first run asks them.
    pub(crate) const ALL: [Self; 8] = [
        Self::KeySource,
        Self::SyncKeySource,
        Self::Database,
        Self::EmbedRole,
        Self::EmbedService,
        Self::RerankRole,
        Self::RerankService,
        Self::Extract,
    ];

    /// The section name the going-back message uses.
    ///
    /// The plan names a *section*, not a question, because the answer being
    /// discarded belongs to a section and an operator who recalls one question is
    /// not told what they lost.
    #[must_use]
    pub(crate) const fn section(self) -> &'static str {
        match self {
            Self::KeySource => "Encryption key",
            Self::SyncKeySource => "Sync key",
            Self::Database => Amend::Database.label(),
            Self::EmbedRole | Self::EmbedService => Amend::Embedding.label(),
            Self::RerankRole | Self::RerankService => Amend::Reranking.label(),
            Self::Extract => Amend::Extraction.label(),
        }
    }

    /// The first step of this section, which is where an amend re-enters it.
    ///
    /// The amend question names a section, not a step, and this is the mapping
    /// between them. `Embedding` enters at the role question rather than at the
    /// endpoint, because a section whose role is local has no endpoint to enter
    /// at and the role question is where a section's answers begin.
    #[must_use]
    pub(crate) const fn first_of(section: Amend) -> Self {
        match section {
            Amend::Database => Self::Database,
            Amend::Embedding => Self::EmbedRole,
            Amend::Reranking => Self::RerankRole,
            Amend::Extraction => Self::Extract,
        }
    }
}

/// Every answer collected so far.
///
/// Whole, and never partially cleared: a question re-asked must keep the previous
/// answer as its default, which is the whole promise of "the last value is the
/// default".
///
/// `PartialEq` because a navigation test asserts that walking back and returning
/// forward reproduces the same answers. Comparing the whole struct is the point:
/// a partial comparison would pass with a lost password.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Answers {
    /// Where the encryption key is kept, once the operator has chosen.
    pub(crate) key_source: Option<KeySource>,
    /// Where the sync key is kept, once the operator has chosen.
    pub(crate) sync_key_source: Option<KeySource>,
    /// Whether multi-host sync is in play for this run, read from the effective
    /// `/sync/omit` pointer before the first question. A single-host deployment
    /// is asked nothing about the sync key.
    pub(crate) sync_in_play: bool,
    /// The database group, once it is finished.
    pub(crate) database: Option<crate::setup_flows_db::DatabaseAnswers>,
    /// The embedding role.
    pub(crate) embed_role: Option<ServiceRole>,
    /// The embedding service, once it is finished.
    pub(crate) embed: Option<crate::setup_services::Service>,
    /// The rerank role.
    pub(crate) rerank_role: Option<ServiceRole>,
    /// The rerank service, once it is finished.
    pub(crate) rerank: Option<crate::setup_services::Service>,
    /// The extraction role.
    pub(crate) extract: Option<ServiceRole>,
}

impl Answers {
    /// The whole step list for these answers, in order.
    ///
    /// The hand-over question is not a step of its own: it is answered inside the
    /// database group, because a failed connection test has to re-enter the fields
    /// and the sub-choice is one keystroke from the first of them. Splitting it
    /// out would put a step between two halves of one group.
    #[must_use]
    pub(crate) fn plan(&self) -> Vec<Step> {
        // The key store is asked once in the life of an install, so the step is
        // here only while there is no record of a choice. A machine that has one
        // never sees the question again, however many times setup is run.
        let mut steps = Vec::new();
        if self.key_source.is_none() {
            steps.push(Step::KeySource);
        }
        // The sync key has the same once-in-the-life rule, and it is asked only
        // where sync is actually in play: a single-host deployment keeps the
        // question out of its run entirely.
        if self.sync_in_play && self.sync_key_source.is_none() {
            steps.push(Step::SyncKeySource);
        }
        steps.extend([Step::Database, Step::EmbedRole]);
        if self.embed_role == Some(ServiceRole::Hosted) {
            steps.push(Step::EmbedService);
        }
        steps.push(Step::RerankRole);
        if self.rerank_role == Some(ServiceRole::Hosted) {
            steps.push(Step::RerankService);
        }
        steps.push(Step::Extract);
        steps
    }

    /// The steps that follow `step`, given these answers.
    #[must_use]
    pub(crate) fn follows(&self, step: Step) -> Vec<Step> {
        let after = Answers::order_of(step);
        self.plan()
            .into_iter()
            .filter(|candidate| Answers::order_of(*candidate) > after)
            .collect()
    }

    /// The steps from `step` onwards, which is the list the loop walks.
    #[must_use]
    pub(crate) fn plan_from(&self, step: Step) -> Vec<Step> {
        let mut steps = vec![step];
        steps.extend(self.follows(step));
        steps
    }

    /// Drop the answers at and above `step`.
    ///
    /// Everything below the step survives, so returning forward re-asks only what
    /// was dropped.
    pub(crate) fn drop_from(&mut self, step: Step) {
        match step {
            // Dropping a key store would make its question come back, which is
            // the one thing a recorded choice must never do. Both key roles
            // follow the rule; their records outlive a back.
            Step::KeySource | Step::SyncKeySource => {}
            Step::Database => self.database = None,
            Step::EmbedRole | Step::EmbedService => {
                self.embed_role = None;
                self.embed = None;
            }
            Step::RerankRole | Step::RerankService => {
                self.rerank_role = None;
                self.rerank = None;
            }
            Step::Extract => self.extract = None,
        }
    }

    /// The answer's index in the full step order, for a step that may not be in
    /// the current list.
    #[must_use]
    pub(crate) fn order_of(step: Step) -> usize {
        Step::ALL
            .iter()
            .position(|candidate| *candidate == step)
            .unwrap_or(0)
    }
}

/// Ask every question and return the collected answers.
///
/// `preflight` runs before the first question, so a contradictory set of flags
/// fails without a single prompt having been shown. A preflight stop is a usage
/// stop: the command line is what is wrong, and the exit code says so. Nothing
/// is written and nothing is started: the caller draws the preview and applies.
///
/// # Errors
/// A refusal, or `q` at any question.
pub(crate) fn drive(
    ctx: &mut Ctx<'_>,
    preflight: impl FnOnce(&Ctx<'_>) -> Result<(), String>,
) -> Result<Answers, Signal> {
    preflight(ctx).map_err(Signal::Usage)?;
    // The walk starts wherever the plan starts, not at a named step. The first
    // step is the key store on a machine that has never chosen one and the
    // database on a machine that has, so naming a step here would either skip the
    // key question forever or ask it on a machine that already answered.
    // The deployment shape is read once, before the plan is built, so the sync
    // key's question is in or out of the whole run rather than appearing at it.
    let answers = Answers {
        sync_in_play: ctx.sync_in_play(),
        ..Answers::default()
    };
    let first = answers.plan().first().copied().unwrap_or(Step::Database);
    walk(ctx, answers, first)
}

/// Walk the plan from `from`, keeping the answers already collected.
///
/// The amend path re-enters a section through here, which is what makes a
/// re-asked question offer the previous answer as its default: the same
/// [`Answers`] the first pass filled is the one the second pass reads.
///
/// # Errors
/// A refusal, `b` at the first step of the walk, or `q` at any question.
pub(crate) fn walk(ctx: &mut Ctx<'_>, mut answers: Answers, from: Step) -> Result<Answers, Signal> {
    let mut steps = answers.plan_from(from);
    let mut at = 0;
    while at < steps.len() {
        let step = steps[at];
        match setup_step_ask::ask_group(ctx, step, &mut answers)? {
            Outcome::Done => {
                // The tail is rebuilt after every answer, so a route change shows
                // up as a step appearing or disappearing rather than as a question
                // about a service the operator declined.
                //
                // The next step is the first rebuilt step that comes *after* the one
                // just answered, in the canonical order. Looking the step up by name
                // and falling back to `at + 1` when it is gone is wrong: a step that
                // has dropped out of the plan shifts every later index down by one,
                // so `at + 1` lands on the step after next. The key store does
                // exactly that — it is answered once and then never appears again —
                // which silently skipped the database question.
                let rebuilt = answers.plan();
                let after = Answers::order_of(step);
                at = rebuilt
                    .iter()
                    .position(|candidate| Answers::order_of(*candidate) > after)
                    .unwrap_or(rebuilt.len());
                steps = rebuilt;
            }
            Outcome::Back => {
                // A back on the first step of the whole run leaves cleanly, because
                // the run has nothing behind it to return to. The first step asks
                // its own first question as `Quit` for the same reason.
                if at == 0 {
                    announce_quit(ctx);
                    return Err(Signal::Quit);
                }
                let previous = steps[at - 1];
                if previous.section() == step.section() {
                    // Inside one section nothing is discarded, so nothing is
                    // announced: the operator asked to change a question, not a
                    // section.
                    at -= 1;
                    continue;
                }
                // The section is named *before* it is dropped, so the choice is
                // informed rather than discovered afterwards.
                let message = going_back(step.section());
                let painted = warn(ctx.palette(), &message);
                let _ = writeln!(ctx.writer(), "  {painted}");
                let why = hint(ctx.palette(), crate::setup_style::nothing_to_undo());
                let _ = writeln!(ctx.writer(), "  {why}");
                let keys = hint(ctx.palette(), crate::setup_render::BACK_HINT);
                let _ = writeln!(ctx.writer(), "  {keys}");
                answers.drop_from(step);
                let rebuilt = answers.plan();
                at = match rebuilt.iter().position(|s| *s == previous) {
                    Some(found) => found,
                    None => at.saturating_sub(1),
                };
                steps = rebuilt;
            }
            Outcome::Quit => {
                announce_quit(ctx);
                return Err(Signal::Quit);
            }
        }
    }
    Ok(answers)
}

/// Print the one line a quit prints.
pub(crate) fn announce_quit(ctx: &mut Ctx<'_>) {
    ctx.confirm(crate::setup_style::quitting());
}

#[cfg(test)]
#[path = "setup_steps_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "setup_steps_sync_tests.rs"]
mod sync_tests;
