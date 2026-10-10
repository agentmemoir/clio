// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the durable statement that a wave claimed a
//! hub before it did the work: the intent's shape and its validation.
//!
//! ## Owns
//! - `DistillIntent` and its validation
//! - The identity a completion write must name
//!
//! ## Does not own
//! - The intent row's SQL (`sqlite_distill_intent` / `postgres_distill_intent`)
//! - The wave's ordering of the claim write and the completion (`clio-write`)
//!
//! ## Boundary
//!
//! Changes to the intent shape or its validation should normally require
//! changes here. Changes to how the row is stored should not require changes
//! here.
//!
//! Keep this module focused on the intent record.

use clio_types::{AmError, ErrorCode};

/// What a wave writes down before it distils a hub, and the revision it records
/// once it has.
///
/// Ids and timestamps only: no content, gist, snapshot, or key material, so the
/// row carries nothing that needs sealing or shredding. An empty
/// `revision_item_id` is the whole of "not finished yet" — there is no state
/// column and no nullable column, so a claim cannot be complete and incomplete
/// at the same time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistillIntent {
    /// Bank isolation key.
    pub bank_id: String,
    /// Hub item id the wave is about to distil.
    pub hub_id: String,
    /// Deterministic unit id the wave writes for that hub.
    pub unit_item_id: String,
    /// The wave's actor.
    pub actor: String,
    /// When the claim was taken: millisecond-epoch text, the format a
    /// derivation record's `distilled_at` uses (`migrate::now_iso`).
    pub claimed_at: String,
    /// Item id of the revision the wave produced; empty until it completes.
    pub revision_item_id: String,
}

impl DistillIntent {
    /// Build an incomplete intent: the completion state starts empty, and only
    /// the completion write ever fills it.
    #[must_use]
    pub fn new(
        bank_id: impl Into<String>,
        hub_id: impl Into<String>,
        unit_item_id: impl Into<String>,
        actor: impl Into<String>,
        claimed_at: impl Into<String>,
    ) -> Self {
        Self {
            bank_id: bank_id.into(),
            hub_id: hub_id.into(),
            unit_item_id: unit_item_id.into(),
            actor: actor.into(),
            claimed_at: claimed_at.into(),
            revision_item_id: String::new(),
        }
    }

    /// Whether the claim has produced its revision. An empty revision id is the
    /// only incomplete state, so this and the stored column cannot disagree.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.revision_item_id.is_empty()
    }

    /// # Errors
    /// Empty bank, hub, unit, actor, or claim time: a claim naming none of
    /// these could never be read back or acted on. A filled revision id: a
    /// claim is stored incomplete, and only the completion write finishes it.
    pub(crate) fn validate(&self) -> Result<(), AmError> {
        if self.bank_id.is_empty() {
            return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
        }
        if self.hub_id.is_empty()
            || self.unit_item_id.is_empty()
            || self.actor.is_empty()
            || self.claimed_at.is_empty()
        {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "distill intent fields are required",
            ));
        }
        if !self.revision_item_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "a claim is stored incomplete; only complete_distill_intent fills its revision",
            ));
        }
        Ok(())
    }

    /// Validate the intent and require it to be the bank's own, so a write can
    /// never land one bank's claim under another.
    ///
    /// # Errors
    /// [`Self::validate`] failures, or a bank that is not the intent's own.
    pub(crate) fn validate_for_bank(&self, bank_id: &str) -> Result<(), AmError> {
        self.validate()?;
        if self.bank_id != bank_id {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "distill intent does not belong to the bank it is written under",
            ));
        }
        Ok(())
    }
}

/// Validate the identity a completion write names.
///
/// # Errors
/// An empty bank is a missing bank, and an empty hub or revision id names
/// nothing to record.
pub(crate) fn validate_completion(
    bank_id: &str,
    hub_id: &str,
    revision_item_id: &str,
) -> Result<(), AmError> {
    if bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if hub_id.is_empty() || revision_item_id.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "distill intent completion needs a hub id and a produced revision id",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "distill_intent_erase_tests.rs"]
mod distill_intent_erase_tests;

#[cfg(test)]
#[path = "distill_intent_tests.rs"]
mod distill_intent_tests;
