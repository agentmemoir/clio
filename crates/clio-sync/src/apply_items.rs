// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the item apply effects: decoding one item
//! payload, the bank and shredded-subject checks, the last-writer-wins update
//! resolution, and the create / invalidate / discard durable writes.
//!
//! ## Owns
//! - The `item` upsert, invalidate, and discard apply effects
//! - The item payload's preserve-on-absent context rule
//!
//! ## Does not own
//! - The apply engine, its dedupe guards, and its dead-letter policy (`apply`)
//! - The record effects (`apply_records`) and the history effects
//!   (`apply_history`)
//! - Item persistence SQL (`clio-store` `Store` primitives)
//! - The subject denial guard that runs before any of these effects
//!   (`denial_guard`)
//!
//! ## Boundary
//!
//! Changes to how one item mutation becomes one durable item row should
//! normally require changes here. Changes to dedupe, trust, or refusal policy
//! should not.
//!
//! Keep this module focused on the item apply effects.

use crate::apply::{ApplyEngine, ApplyOutcome, dead_letter, lww_loser};
use crate::protocol::{SyncContent, SyncMutation};
use clio_types::{AmError, ErrorCode, MemoryItem};

/// Apply one `item` upsert: create the item, or update it when the incoming
/// stamp wins the LWW comparison.
///
/// # Errors
/// Storage failures (retryable); schema and policy rejects become
/// `DeadLettered`.
pub(crate) fn apply_item_upsert(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<ApplyOutcome, AmError> {
    let SyncContent::Plain(v) = &m.content else {
        return Ok(ApplyOutcome::Applied);
    };
    let mut item: MemoryItem = match serde_json::from_value(v.clone()) {
        Ok(item) => item,
        Err(e) => {
            return crate::apply_history::reject(engine, m, envelope_json, "bad item content", &e);
        }
    };
    if item.bank_id != m.bank {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::Forbidden,
                "mutation bank does not match item bank",
            ),
        );
    }
    if engine.sync.subject_is_shredded(&item.subject_id)? {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::ErasedSubject,
                "erased subject content resurrection",
            ),
        );
    }
    if let Err(err) = item.validate() {
        return dead_letter(engine, m, envelope_json, &err);
    }
    if let Some(existing) = engine.store.get_memory_item(&m.bank, &m.entity_id)? {
        if lww_loser(engine, m, &existing.updated_at, existing.admission_score)? {
            return Ok(ApplyOutcome::ConflictLoser);
        }
        // Preserve-on-absent: a legacy peer payload
        // without a `context` field (absent or null) never clears the stored
        // context; a present value replaces it after validation.
        match v.get("context") {
            None | Some(serde_json::Value::Null) => item.context.clone_from(&existing.context),
            Some(serde_json::Value::String(context)) => {
                clio_types::validate_context(context)?;
            }
            _ => {}
        }
        engine.store.update_memory_item(&item, "sync-apply")?;
    } else {
        engine.store.create_memory_item(&item, "sync-apply")?;
    }
    Ok(ApplyOutcome::Applied)
}

/// Apply one `item` invalidate: close both time axes of the stored row.
///
/// # Errors
/// Storage failures (retryable).
pub(crate) fn apply_item_invalidate(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    _envelope_json: &str,
) -> Result<ApplyOutcome, AmError> {
    if engine
        .store
        .get_item_identity(&m.bank, &m.entity_id)?
        .is_none()
    {
        return Ok(ApplyOutcome::Applied);
    }
    let ended = engine.store.invalidate_item(
        &m.bank,
        &m.entity_id,
        &m.transaction_time,
        &m.transaction_time,
        None,
        "sync-apply",
    )?;
    if ended.closed_ids.is_empty() {
        return Ok(ApplyOutcome::Duplicate);
    }
    Ok(ApplyOutcome::Applied)
}

/// Apply one `item` discard: the documented discard path, whose repeat is a
/// duplicate rather than a second effect.
///
/// # Errors
/// Storage failures (retryable); a refused discard becomes `DeadLettered`.
pub(crate) fn apply_item_discard(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<ApplyOutcome, AmError> {
    if engine
        .store
        .get_item_identity(&m.bank, &m.entity_id)?
        .is_none()
    {
        return Ok(ApplyOutcome::Applied);
    }
    let identity = engine.store.discard_item(
        &m.bank,
        &m.entity_id,
        "sync discard",
        &m.transaction_time,
        "sync-apply",
    );
    match identity {
        Ok(_) => Ok(ApplyOutcome::Applied),
        Err(err) if err.message.contains("already discarded") => Ok(ApplyOutcome::Duplicate),
        Err(err) => dead_letter(engine, m, envelope_json, &err),
    }
}
