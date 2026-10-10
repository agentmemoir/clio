// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//
//! This module is responsible for choosing the per-hit display form and text that enter a composed context pack, and for tallying how many hits took each form.
//
//! ## Owns
//! - The total hit-form selector: the fidelity channel's preference between a rendering, a gist, and a marked reference, plus the budget fallback among them
//! - The fixed reference marker that keeps a reference from looking like content
//! - The per-form tally fold that is the single source for the pack's form counts
//
//! ## Does not own
//! - Budget arithmetic, section packing, or the compose request itself (`compose.rs`)
//! - The token formula (`token.rs`, formula lives in `clio-types::tokens`); the fallback here charges through that same estimator
//
//! ## Boundary
//
//! Changes to which form a hit gets, or how references are marked and counted, should normally require changes here. Changes to budget handling, pack assembly, or the token formula should not require changes here.
//!
//! Keep this module focused on per-hit form selection and form tallying.

use crate::token::estimate_tokens;
use clio_config::retention_levels::PackFidelityDefault;
use serde::Serialize;

/// Marks that the text following this prefix is a reference to a stored item, not content.
///
/// The marker is a fixed prefix plus an identifier, so it can never carry content from the pack. The selector below is the only producer of marked text, and the same input always renders the same marked text.
pub const REFERENCE_MARKER: &str = "[reference] ";

/// The form a hit takes in a composed pack.
///
/// `Prose` is the hit's gist. `Exact` is the hit's bounded rendering, which the
/// `exact` channel prefers and `flow` takes when the hit has no gist.
/// `Reference` is a marked pointer to the snapshot or the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HitForm {
    /// The hit contributes its gist text.
    Prose,
    /// The hit contributes its bounded rendering; `exact` prefers it and `flow`
    /// takes it when the hit has no gist.
    Exact,
    /// The hit contributes a marked reference to the snapshot or the item.
    Reference,
}

impl HitForm {
    /// The stable lowercase name, matching the serde serialization.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prose => "prose",
            Self::Exact => "exact",
            Self::Reference => "reference",
        }
    }
}

/// Chooses the form and the text a hit contributes to the pack.
///
/// The channel is the only difference between the two precedences: `flow` reads the gist first and `exact` reads the rendering first, and both then read the other half. Either way a half that is absent or empty is skipped, and the hit ends at a reference whose text is the snapshot reference when present and the item id otherwise, always prefixed with [`REFERENCE_MARKER`].
///
/// `remaining_tokens` is the memory budget left at this hit's turn. A rendering that would not fit it is dropped whole for that hit, which then contributes the marked reference; nothing is ever shortened to fit. The selector is total: every input returns a form, the returned form is the form of the returned text, and no form's text is a bare identifier.
#[must_use]
pub fn hit_form_text(
    channel: PackFidelityDefault,
    remaining_tokens: usize,
    gist: Option<&str>,
    authoritative: Option<&str>,
    snapshot_ref: Option<&str>,
    item_id: &str,
) -> (HitForm, String) {
    let reference = || {
        (
            HitForm::Reference,
            format!("{REFERENCE_MARKER}{}", snapshot_ref.unwrap_or(item_id)),
        )
    };
    let gist = gist.filter(|g| !g.is_empty());
    let render = authoritative.filter(|a| !a.is_empty());
    match channel {
        // The `flow` precedence does not throw a rendering away: with no gist
        // to prefer, the rendering is the only text the hit has, so it takes it
        // and no budget is charged. A composed pack never reaches that line,
        // because the pack asks for a rendering only under `exact` and packs
        // only the hits of the same request; a direct call to this selector can
        // still carry one, and the total selector answers it either way.
        PackFidelityDefault::Flow => match (gist, render) {
            (Some(gist), _) => (HitForm::Prose, gist.to_owned()),
            (None, Some(render)) => (HitForm::Exact, render.to_owned()),
            (None, None) => reference(),
        },
        // The one budget decision point: a rendering that does not fit the
        // remaining budget is dropped whole and the hit reports the reference it
        // falls back to, so the form record and the text cannot disagree.
        PackFidelityDefault::Exact => match render {
            Some(render) if estimate_tokens(render) <= remaining_tokens => {
                (HitForm::Exact, render.to_owned())
            }
            Some(_) => reference(),
            None => match gist {
                Some(gist) => (HitForm::Prose, gist.to_owned()),
                None => reference(),
            },
        },
    }
}

/// Counts how many hits took each form across a pack.
///
/// This is the single source for the pack's per-form tally; callers must not count forms a second time. The tally carries counts only — never ids and never text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct FormTally {
    /// Hits that contributed their gist.
    pub prose: usize,
    /// Hits that contributed their bounded rendering.
    pub exact: usize,
    /// Hits that contributed a marked reference.
    pub reference: usize,
}

impl FormTally {
    /// Folds an iterator of forms into one tally.
    #[must_use]
    pub fn fold(forms: impl Iterator<Item = HitForm>) -> Self {
        let mut tally = Self::default();
        for form in forms {
            match form {
                HitForm::Prose => tally.prose += 1,
                HitForm::Exact => tally.exact += 1,
                HitForm::Reference => tally.reference += 1,
            }
        }
        tally
    }
}

#[cfg(test)]
#[path = "pack_forms_tests.rs"]
mod pack_forms_tests;
