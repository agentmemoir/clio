// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the in-process `discard(item_id, reason)`
//! operations-removal tool with confirmation semantics.
//!
//! ## Owns
//! - Argument validation (`item_id` + non-empty `reason`), the confirm gate, and
//!   the structured dry-run vs confirmed outcome
//! - PII-safe telemetry on the confirmed mutating path
//!
//! ## Does not own
//! - The store `UPDATE items SET discarded_at, discard_reason` (see `clio-store`)
//! - Transport-level confirmation plumbing (MCP binding) and compliance
//!   erasure or hygiene noise scoring (separate paths)
//!
//! ## Boundary
//!
//! Changes to discard validation, confirmation, or telemetry should normally require changes here.
//! Changes to store SQL or key/content handling should not require changes here.
//!
//! Keep this module focused on the discard tool contract.

use clio_store::{ItemIdentity, Store};
use clio_types::{AmError, ErrorCode, OpContext};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Telemetry for one confirmed `discard`.
///
/// Deliberately carries no `reason` text (it may contain personal data); only
/// the reason's length is logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardEvent {
    /// Always `discard`.
    pub operation: String,
    /// Bank isolation key.
    pub bank: String,
    /// Attribution actor.
    pub actor: String,
    /// Caller timestamp.
    pub timestamp: String,
    /// Removed item id.
    pub item_id: String,
    /// Semantic category, when the item is semantic.
    pub category: Option<String>,
    /// `fact` or `belief`.
    pub epistemic_kind: String,
    /// Length of the operator-supplied reason (never the text).
    pub reason_len: usize,
}

/// Sink for discard telemetry.
pub trait DiscardSink: std::fmt::Debug {
    /// Record one event.
    fn emit(&self, event: DiscardEvent);
}

/// In-memory sink for tests.
#[derive(Debug, Default)]
pub struct DiscardVecSink {
    events: Mutex<Vec<DiscardEvent>>,
}

impl DiscardVecSink {
    /// Snapshot of recorded events.
    #[must_use]
    pub fn events(&self) -> Vec<DiscardEvent> {
        self.events.lock().expect("discard sink poisoned").clone()
    }
}

impl DiscardSink for DiscardVecSink {
    fn emit(&self, event: DiscardEvent) {
        self.events
            .lock()
            .expect("discard sink poisoned")
            .push(event);
    }
}

/// Inputs for the in-process `discard` tool.
pub struct DiscardArgs<'a> {
    /// Discrete repository.
    pub store: &'a dyn Store,
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// Target item id.
    pub item_id: &'a str,
    /// Operator-supplied removal reason (required, non-empty).
    pub reason: &'a str,
    /// `false` (default) = dry-run; `true` = perform the removal.
    pub confirm: bool,
    /// Removal timestamp (UTC), used for `discarded_at` and telemetry.
    pub timestamp: &'a str,
    /// Discard telemetry sink.
    pub sink: &'a dyn DiscardSink,
}

/// Outcome of a `discard` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscardOutcome {
    /// Unconfirmed: zero writes; the resolved target identity is returned so the
    /// caller can preview exactly what would be removed.
    DryRun(ItemIdentity),
    /// Confirmed: the target was marked discarded.
    Discarded(ItemIdentity),
}

/// In-process `discard(item_id, reason, confirm?)`.
///
/// Without `confirm` this is a dry-run: it resolves the bank-scoped target and
/// returns a would-discard response with zero writes and no telemetry. With
/// `confirm` it performs the store removal and emits one event. The path
/// never touches content payloads, DEKs, or supersession edges; discard is not
/// compliance erasure and never satisfies `erase_request`.
///
/// Repeat-discard is rejected as `InvalidArgument` on the confirmed path
/// (documented policy); a dry-run preview of an already-discarded item is
/// rejected as `NotFound` because only active items resolve an identity.
///
/// # Errors
/// Missing bank, empty `item_id`/`reason`, or not-found / already-discarded
/// store rejections.
pub fn discard(args: &DiscardArgs<'_>) -> Result<DiscardOutcome, AmError> {
    if args.ctx.bank.as_str().is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if args.item_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item_id is required",
        ));
    }
    let reason = args.reason.trim();
    if reason.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "reason is required and must be non-empty",
        ));
    }
    let bank = args.ctx.bank.as_str();
    let outcome = if args.confirm {
        let identity = args.store.discard_item(
            bank,
            args.item_id,
            reason,
            args.timestamp,
            args.ctx.actor.as_str(),
        )?;
        args.sink.emit(DiscardEvent {
            operation: "discard".into(),
            bank: bank.into(),
            actor: args.ctx.actor.as_str().into(),
            timestamp: args.timestamp.into(),
            item_id: identity.item_id.clone(),
            category: identity.category.clone(),
            epistemic_kind: identity.epistemic_kind.clone(),
            reason_len: reason.chars().count(),
        });
        DiscardOutcome::Discarded(identity)
    } else {
        let identity = args
            .store
            .get_item_identity(bank, args.item_id)?
            .ok_or_else(|| AmError::new(ErrorCode::NotFound, not_found(args.item_id)))?;
        DiscardOutcome::DryRun(identity)
    };
    Ok(outcome)
}

/// Not-found message shared by the dry-run and store paths (no bank leak).
fn not_found(item_id: &str) -> String {
    format!("item `{item_id}` not found in bank")
}

#[cfg(test)]
#[path = "discard_tests.rs"]
mod discard_tests;
