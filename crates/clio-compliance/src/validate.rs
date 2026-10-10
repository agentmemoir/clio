// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the collaborative reviewer `validate` tool
//! service: routing reviewer actions to the underlying mutators
//! with attributable audit logging.
//!
//! ## Owns
//! - `ValidateAction` enum (`attest` | `update` | `invalidate` | `discard`,
//!   with `delete` accepted as a documented alias for `discard`)
//! - Action routing: attest → durable attestation telemetry; update →
//!   auditable `correct` (respects the item's `update_rule`); invalidate →
//!   bi-temporal close; discard → logged operations removal
//! - Fail-closed guard: destructive/corrective actions require a non-empty note
//! - The presenting credential's label inside each durable `validate_*` row
//!
//! ## Does not own
//! - The underlying mutator semantics (`clio-write` / `clio-compliance::correct`)
//! - Ops health `verify` — a different `validate` vocabulary entirely
//!
//! ## Boundary
//!
//! Changes to reviewer action routing should normally require changes here.
//! Changes to the mutators or MCP binding should not require changes here.
//!
//! Keep this module focused on the reviewer validation service.

use clio_store::{ContinuousStore, Store};
use clio_types::{Actor, AmError, ErrorCode, OpContext, PrincipalLabel, record_principal};
use serde::Serialize;
use serde_json::json;

/// Reviewer action (`validate.action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidateAction {
    /// Record a reviewer attestation (telemetry only; content unchanged).
    Attest,
    /// Auditable correction via the `correct` service (rule-respecting).
    Update,
    /// Close validity on both time axes (supersession; nothing deleted).
    Invalidate,
    /// Logged operations removal from active use (never compliance erasure).
    Discard,
}

impl ValidateAction {
    /// Parse the wire value.
    ///
    /// # Errors
    /// Unknown action label.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "attest" => Ok(Self::Attest),
            "update" => Ok(Self::Update),
            "invalidate" => Ok(Self::Invalidate),
            // `delete` is the requirement's label; it aliases the
            // ops-removal `discard` action (never compliance erasure).
            "discard" | "delete" => Ok(Self::Discard),
            _other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "validate action must be attest|update|invalidate|discard (or its alias `delete`), got `{raw}`"
                ),
            )),
        }
    }

    /// Stable wire label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Attest => "attest",
            Self::Update => "update",
            Self::Invalidate => "invalidate",
            Self::Discard => "discard",
        }
    }
}

/// Inputs for one reviewer validation.
pub struct ValidateArgs<'a> {
    /// Discrete repository.
    pub store: &'a dyn Store,
    /// Bank isolation key.
    pub bank_id: &'a str,
    /// Reviewer attribution (non-empty).
    pub actor: &'a str,
    /// Credential that presented the request; `None` on an unbound transport.
    pub principal: Option<&'a PrincipalLabel>,
    /// Target memory item id.
    pub item_id: &'a str,
    /// Reviewer action.
    pub action: ValidateAction,
    /// Review observation; required non-empty for update/invalidate/discard.
    pub note: Option<&'a str>,
    /// Corrected value; required for `update`.
    pub new_content: Option<&'a str>,
    /// Review timestamp (closes axes / stamps telemetry).
    pub timestamp: &'a str,
    /// Caller-supplied id for the corrected assertion (update path).
    pub new_item_id: &'a str,
    /// Continuous repository (item corrections vs continuous attribute
    /// adjustments route by the target's declared `update_rule`).
    pub continuous: &'a dyn ContinuousStore,
}

/// Structured result of one reviewer action.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidateOutcome {
    /// Target item id.
    pub item_id: String,
    /// Action that was applied.
    pub action: &'static str,
    /// Reviewer attribution.
    pub actor: String,
    /// Non-empty reviewer note (masked before storage).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Whether the durable state changed (attest never changes content).
    pub applied: bool,
}

/// Route one reviewer action with attributable audit logging.
///
/// Destructive/corrective actions (`update`, `invalidate`, `discard`) fail
/// closed without a non-empty `note`; every action leaves a durable
/// `validate_*` audit row carrying actor, action, and timestamp.
///
/// # Errors
/// Missing targets/notes, unknown ids, or store failures.
#[allow(clippy::too_many_lines)]
pub fn validate(args: &ValidateArgs<'_>) -> Result<ValidateOutcome, AmError> {
    if args.bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    let _ = Actor::parse(args.actor)?; // attribution must be a known actor kind
    if args.item_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item_id is required",
        ));
    }
    // Mask secrets once at entry so the durable audit detail and the returned
    // outcome both carry the scrubbed note (module contract).
    let scrubbed = args.note.map(clio_config::secret::scrub_inline_secrets);
    let note = scrubbed.as_deref();
    let outcome = match args.action {
        ValidateAction::Attest => {
            args.store
                .get_item_identity(args.bank_id, args.item_id)?
                .ok_or_else(|| {
                    AmError::new(
                        ErrorCode::ItemNotFound,
                        format!("item `{}` not found in bank", args.item_id),
                    )
                })?;
            let mut detail = json!({
                "action": "attest",
                "note": note,
                "timestamp": args.timestamp
            });
            record_principal(&mut detail, args.principal);
            args.store.record_audit_event(
                args.bank_id,
                "validate_attest",
                Some(args.item_id),
                args.actor,
                &detail,
            )?;
            ValidateOutcome {
                item_id: args.item_id.to_owned(),
                action: "attest",
                actor: args.actor.to_owned(),
                note: note.map(str::to_owned),
                applied: false,
            }
        }
        ValidateAction::Update => {
            let (note, new_content) = required_pair(args.action, note, args.new_content)?;
            correct_update(args, note, new_content)?;
            ValidateOutcome {
                item_id: args.item_id.to_owned(),
                action: "update",
                actor: args.actor.to_owned(),
                note: Some(note.to_owned()),
                applied: true,
            }
        }
        ValidateAction::Invalidate => {
            let note = required_note(args.action, note)?;
            let ctx = OpContext::new(
                clio_types::Bank::new(args.bank_id),
                Actor::parse(args.actor)?,
            );
            let outcome = clio_write::triple::invalidate(
                args.store,
                &ctx,
                args.item_id,
                None,
                args.timestamp,
                None,
                &clio_write::TripleVecSink::default(),
            )?;
            let mut detail = json!({
                "action": "invalidate",
                "note": note,
                "closed_ids": outcome.closed_ids,
                "timestamp": args.timestamp
            });
            record_principal(&mut detail, args.principal);
            args.store.record_audit_event(
                args.bank_id,
                "validate_invalidate",
                Some(args.item_id),
                args.actor,
                &detail,
            )?;
            ValidateOutcome {
                item_id: args.item_id.to_owned(),
                action: "invalidate",
                actor: args.actor.to_owned(),
                note: Some(note.to_owned()),
                applied: !outcome.already_ended,
            }
        }
        ValidateAction::Discard => {
            let note = required_note(args.action, note)?;
            let ctx = OpContext::new(
                clio_types::Bank::new(args.bank_id),
                Actor::parse(args.actor)?,
            );
            let d_args = clio_write::DiscardArgs {
                store: args.store,
                ctx: &ctx,
                item_id: args.item_id,
                reason: note,
                confirm: true,
                timestamp: args.timestamp,
                sink: &clio_write::DiscardVecSink::default(),
            };
            clio_write::discard(&d_args)?;
            let mut detail = json!({
                "action": "discard",
                "note": note,
                "timestamp": args.timestamp
            });
            record_principal(&mut detail, args.principal);
            args.store.record_audit_event(
                args.bank_id,
                "validate_discard",
                Some(args.item_id),
                args.actor,
                &detail,
            )?;
            ValidateOutcome {
                item_id: args.item_id.to_owned(),
                action: "discard",
                actor: args.actor.to_owned(),
                note: Some(note.to_owned()),
                applied: true,
            }
        }
    };
    Ok(outcome)
}

/// Required note for destructive/corrective reviewer actions.
fn required_note(action: ValidateAction, note: Option<&str>) -> Result<&str, AmError> {
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    note.ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "action `{}` requires a non-empty reviewer note",
                action.as_str()
            ),
        )
    })
}

/// Required note + new content for the `update` route.
fn required_pair<'a>(
    action: ValidateAction,
    note: Option<&'a str>,
    new_content: Option<&'a str>,
) -> Result<(&'a str, &'a str), AmError> {
    let note = required_note(action, note)?;
    let new_content = new_content
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::InvalidArgument,
                "action `update` requires new_content",
            )
        })?;
    Ok((note, new_content))
}

/// Run the reviewer `update` route through the auditable `correct` service.
fn correct_update(args: &ValidateArgs<'_>, note: &str, new_content: &str) -> Result<(), AmError> {
    let c_args = super::CorrectArgs {
        store: args.store,
        continuous: args.continuous,
        bank_id: args.bank_id,
        actor: args.actor,
        item_id: args.item_id,
        new_value: new_content,
        reason: note,
        // Reviewer `update` preserves the existing context; an explicit
        // replacement goes through the `correct` tool.
        context: None,
        timestamp: args.timestamp,
        new_item_id: args.new_item_id,
    };
    super::correct(&c_args).map(|_| ())
}

#[cfg(test)]
#[path = "validate_test_support.rs"]
mod validate_test_support;

#[cfg(test)]
#[path = "validate_tests.rs"]
mod validate_tests;

#[cfg(test)]
#[path = "validate_principal_tests.rs"]
mod validate_principal_tests;
