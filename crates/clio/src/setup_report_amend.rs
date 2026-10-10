// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two questions that come after the preview:
//! which section to re-ask when the operator amends, and the typed `switch` a
//! destructive database change asks for.
//!
//! ## Owns
//! - `Amend`, the four sections and the names the list shows
//! - `amend_section`, the menu of sections to re-ask
//! - `switch`, the typed confirmation and the refusal that replaces it
//!
//! ## Does not own
//! - The preview itself (see `setup_report`)
//! - Re-asking a section's questions (see `setup_step_ask`)
//! - Whether the store is non-empty, and the counts (stage 6 supplies both)
//!
//! ## Boundary
//!
//! Changes to what the post-preview questions say, or to how many attempts the
//! typed confirmation allows, should normally require changes here. Changes to
//! what the preview shows should not require changes here.
//!
//! Keep this module focused on the last two questions of a run.
//!
//! # Why amending jumps to a section rather than walking back
//!
//! Walking back eight questions to fix a port is not an amendment, it is a
//! punishment. Naming the section lets the operator jump straight to it, and the
//! names are the same ones the going-back message uses, so the vocabulary is one
//! word per section across the whole wizard. Going back one question at a time is
//! still available from every question, and this is the shortcut, not the only
//! way.
//!
//! # Why `switch` is typed and not Enter
//!
//! Pointing Clio at a different database does not move data: every memory in the
//! old store becomes unreachable, with no error, because an empty database is a
//! healthy database. That is the one action in this wizard an Enter keypress
//! should not authorize, and typing a word is the smallest barrier that stops a
//! reflexive Enter.

use crate::setup_cli_flows::Ctx;
use crate::setup_prompt::Choice;
use crate::setup_render::Signal;
use crate::setup_style::error;

/// Which section an amend request named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Amend {
    /// Re-ask the database group.
    Database,
    /// Re-ask the embedding questions.
    Embedding,
    /// Re-ask the rerank questions.
    Reranking,
    /// Re-ask the extraction question.
    Extraction,
}

impl Amend {
    /// Every section, in the order `a` lists them.
    pub(crate) const ALL: [Self; 4] = [
        Self::Database,
        Self::Embedding,
        Self::Reranking,
        Self::Extraction,
    ];

    /// The word the list and the going-back message use.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Database => "Database",
            Self::Embedding => "Embedding",
            Self::Reranking => "Reranking",
            Self::Extraction => "Extraction",
        }
    }

    /// The word a menu item carries for this section.
    #[must_use]
    pub(crate) const fn value(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::Embedding => "embedding",
            Self::Reranking => "reranking",
            Self::Extraction => "extraction",
        }
    }

    /// Read a section name, case-insensitively.
    #[must_use]
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        Self::ALL.into_iter().find(|section| {
            section.label().eq_ignore_ascii_case(raw) || section.value().eq_ignore_ascii_case(raw)
        })
    }
}

/// Ask which section to re-ask, returning `None` when the operator backed out.
///
/// Backing out means "not now" rather than "leave": the preview is still on
/// screen and the operator may want to apply it as it stands, so backing out
/// returns to the question above rather than ending the run.
///
/// # Errors
/// The question could not be asked, or the operator left.
pub(crate) fn amend_section(ctx: &mut Ctx<'_>, offered: &[Amend]) -> Result<Option<Amend>, Signal> {
    // With nothing to change there is nothing to ask. A menu with no options would
    // have to invent a default, and whatever it invented would name a section the
    // operator never answered — so the run applies the answers it has instead.
    if offered.is_empty() {
        return Ok(None);
    }
    let options: Vec<Choice> = offered
        .iter()
        .copied()
        .map(|section| {
            let help = match section {
                Amend::Database => "where the memories live",
                Amend::Embedding => "where embeddings come from",
                Amend::Reranking => "where reranking comes from",
                Amend::Extraction => "where extraction comes from",
            };
            Choice::new(section.value(), section.label(), help)
        })
        .collect();
    // The first offered section is the default, and it is by construction one this
    // run offered: the menu refuses anything outside its own options, so the value
    // that arrives here is always a section from `offered`.
    let default = offered[0].value();
    match ctx.ask_choice("Which answer do you want to change?", &options, default)? {
        crate::setup_render::Reply::Back => Ok(None),
        crate::setup_render::Reply::Value(typed) => {
            Amend::parse(&typed).map(Some).ok_or_else(|| {
                Signal::Refused(format!(
                    "`{typed}` is not a section you can change; \
                     the ones you can are: {}",
                    offered
                        .iter()
                        .map(|section| section.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
        }
    }
}

/// How many times `switch` may be mistyped before the run refuses.
///
/// Three, because the number is in the plan and changing it would change the
/// transcripts, and because an operator who has mistyped a six-letter word three
/// times has not read the question rather than been unlucky.
const SWITCH_ATTEMPTS: usize = 3;

/// What changing the database does, said in the order an operator needs it.
///
/// One spelling for the preview, the notice under it, and the `switch`
/// confirmation, so the wizard cannot describe the same action in two ways.
///
/// Three facts, in this order, because that is the order the operator meets them:
/// where Clio will read from afterwards, what that costs them, and what is *not*
/// happening. The last one is the one that stops the warning being read as "my
/// memories are about to be deleted", which is the reading every earlier wording
/// invited and none of them said was wrong.
///
/// Nothing here says "changes where it looks" or "does not move data". Both are
/// true and neither tells the operator anything they can act on.
#[must_use]
pub(crate) fn not_moving_data() -> &'static str {
    "After this, Clio reads and writes the new database instead, and that one is empty, so those \
     memories will not show up in Clio. Nothing is deleted and nothing is copied: the old \
     database keeps everything, and pointing Clio back at it brings them back."
}

/// The `switch` confirmation a destructive apply needs.
///
/// `reason` names the consequence in the operator's own terms — how many
/// memories and subjects are in the store being left behind, and that Clio will
/// not read it afterwards. It is printed in full every time, because an operator
/// who reaches this prompt on the third attempt has forgotten the first.
///
/// # Errors
/// The stream could not be written or read, or the attempts ran out.
#[allow(dead_code, reason = "the stage 6 apply path calls it")]
pub(crate) fn switch(ctx: &mut Ctx<'_>, reason: &str) -> Result<(), Signal> {
    let palette = ctx.palette();
    let _ = writeln!(
        ctx.writer(),
        "  {}",
        crate::setup_style::warn(
            palette,
            &format!(
                "{reason}\n  {}\n  Type `switch` to point Clio at the new one.",
                not_moving_data()
            )
        )
    );
    for attempt in 1..=SWITCH_ATTEMPTS {
        // The question itself is the sentence printed above, so the field carries a
        // label that says where to type and nothing more. The default is the empty
        // string, which makes a bare Enter come back as an empty answer rather than
        // as a usage stop: Enter is the most likely thing to be typed by reflex, and
        // it has to re-ask rather than end the run.
        match ctx.ask("Type here", Some("")) {
            Ok(crate::setup_render::Reply::Back) => {
                return Err(Signal::Quit);
            }
            // Case-insensitively, and for the same reason the section names are: a
            // word the wizard itself printed, and the cost of refusing a capital
            // letter is three more attempts at a destructive action. Nothing is
            // protected by the spelling — the guard is the word, not its case.
            Ok(crate::setup_render::Reply::Value(typed))
                if typed.trim().eq_ignore_ascii_case("switch") =>
            {
                return Ok(());
            }
            Ok(_) => {
                let left = SWITCH_ATTEMPTS - attempt;
                let again = if left == 0 {
                    "Nothing was changed.".to_owned()
                } else {
                    format!("{left} attempt{} left.", if left == 1 { "" } else { "s" })
                };
                let _ = writeln!(
                    ctx.writer(),
                    "  {}",
                    error(
                        palette,
                        &format!(
                            "`switch` was not typed, so the database was not changed. {again}"
                        )
                    )
                );
            }
            Err(Signal::Quit) => return Err(Signal::Quit),
            Err(stop) => return Err(stop),
        }
    }
    Err(Signal::Refused(
        "the confirmation was not typed, so nothing was changed".to_owned(),
    ))
}

#[cfg(test)]
#[path = "setup_report_amend_tests.rs"]
mod tests;
