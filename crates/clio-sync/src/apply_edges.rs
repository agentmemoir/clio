// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the assoc-edge and triple apply effects: the
//! edge upsert and reinforce, the edge prune, and the triple invalidate.
//!
//! ## Owns
//! - The assoc edge effect, including its referential-target check and its
//!   last-writer-wins comparison
//! - The triple invalidate effect
//!
//! ## Does not own
//! - The apply order these effects run in (see `apply`)
//! - The item, record, and history effects (see `apply_items`,
//!   `apply_records`, `apply_history`)
//! - Quarantine policy and reason codes (see `apply_dead_letter`)
//!
//! ## Boundary
//!
//! Changes to edge or triple apply behavior should normally require changes
//! here. Changes to the apply order should not require changes here.
//!
//! Keep this module focused on the edge and triple effects.

use super::{ApplyEngine, ApplyOutcome, dead_letter, lww_loser};
use crate::protocol::{SyncContent, SyncMutation};
use clio_store::AssocEdgeRecord;
use clio_types::{AmError, ErrorCode};

pub(super) fn apply_assoc(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
    prune: bool,
) -> Result<ApplyOutcome, AmError> {
    let SyncContent::Plain(v) = &m.content else {
        return Ok(ApplyOutcome::Applied);
    };
    let edge: AssocEdgeRecord = match serde_json::from_value(v.clone()) {
        Ok(edge) => edge,
        Err(e) => {
            return crate::apply_history::reject(engine, m, envelope_json, "bad assoc content", &e);
        }
    };
    if edge.bank_id != m.bank {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::Forbidden,
                "mutation bank does not match edge bank",
            ),
        );
    }
    // Referential targets: both endpoints must exist locally (matrix: validate
    // referential targets). Erased endpoints regenerate via erase hooks.
    for endpoint in [&edge.source_id, &edge.target_id] {
        if engine.store.get_item_identity(&m.bank, endpoint)?.is_none() {
            return dead_letter(
                engine,
                m,
                envelope_json,
                &AmError::new(ErrorCode::NotFound, "assoc endpoint missing locally"),
            );
        }
    }
    if prune {
        engine.store.delete_assoc_edge(&m.bank, &m.entity_id)?;
        return Ok(ApplyOutcome::Applied);
    }
    if let Some(existing) = engine.store.get_assoc_edge(&m.bank, &edge.id)?
        && lww_loser(engine, m, &existing.last_updated_at, None)?
    {
        return Ok(ApplyOutcome::ConflictLoser);
    }
    engine.store.put_assoc_edge(&edge)?;
    Ok(ApplyOutcome::Applied)
}

pub(super) fn apply_triple_invalidate(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<ApplyOutcome, AmError> {
    let SyncContent::Plain(v) = &m.content else {
        return Ok(ApplyOutcome::Applied);
    };
    let subject = v
        .get("subject")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let predicate = v
        .get("predicate")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let object = v.get("object").and_then(serde_json::Value::as_str);
    if subject.is_empty() || predicate.is_empty() {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::InvalidArgument,
                "triple invalidate needs identity",
            ),
        );
    }
    let ended = engine.store.triple_end(
        &m.bank,
        subject,
        predicate,
        object,
        &m.transaction_time,
        &m.transaction_time,
    )?;
    if ended.closed_ids.is_empty() && !ended.already_ended {
        return Ok(ApplyOutcome::Duplicate);
    }
    Ok(ApplyOutcome::Applied)
}

#[cfg(test)]
#[path = "apply_edges_tests.rs"]
mod apply_edges_tests;
