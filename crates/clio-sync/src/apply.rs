// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync apply engine:
//! idempotent dedupe on `sync_mutation_id`, the `remote_admission_trust`
//! apply matrix, the default LWW conflict rule, erased-subject fail-closed,
//! and the sync dead-letter quarantine.
//!
//! ## Owns
//! - `ApplyEngine` and the per-op apply matrix
//! - LWW resolution + conflict journal recording
//! - The journal self-heal contract (see `ApplyOutcome::Applied` / `Duplicate`)
//! - Re-exports of the dead-letter quarantine helpers (defined in
//!   `apply_dead_letter`)
//!
//! ## Does not own
//! - Cursor advance policy (see `client` — cursors advance only after durable
//!   apply or durable quarantine)
//! - Transport (`server` / `client`)
//! - Local admission scoring: trusted peer creates apply without θ re-scoring
//!   by design (schema/category/authz/erasure checks only)
//!
//! ## Boundary
//!
//! Changes to apply semantics or conflict behavior should normally require
//! changes here. Changes to transport or cursor persistence should not
//! require changes here.
//!
//! Keep this module focused on the sync apply engine.

use crate::apply_records;
use crate::clock::now_utc;
use crate::journal_row::journal_row as build_journal_row;
use crate::protocol::{
    ENTITY_KIND_BELIEF, ENTITY_KIND_FAILURE, ENTITY_KIND_ITEM, ENTITY_KIND_PERSONA,
    ENTITY_KIND_TASK, ENTITY_KIND_TRIPLE, OP_DISCARD, OP_INVALIDATE, OP_PRUNE, OP_REINFORCE,
    OP_UPSERT, SyncContent, SyncMutation,
};
use clio_store::{
    AssocEdgeRecord, DeadLetterStatus, DekProvider, Store, SyncConflictRow, SyncStore,
};
use clio_types::{AmError, ErrorCode, MemoryItem};

#[path = "apply_dead_letter.rs"]
mod apply_dead_letter;

pub use apply_dead_letter::quarantine;

pub(crate) use apply_dead_letter::dead_letter;

/// Outcome of applying one sync mutation (transitions).
#[derive(Debug, Clone, PartialEq)]
pub enum ApplyOutcome {
    /// Durable apply done (created or updated the durable row), or an
    /// equal-state redelivery whose durable row already existed: the engine
    /// journals the mutation either way, so the row becomes relayable via
    /// pull (journal self-heal).
    Applied,
    /// Idempotent hit caught by the pre-checked guards (`journal_get`, an
    /// operator-acked skip, or the content-hash guard) or an item/triple
    /// no-op branch: no durable effect and nothing journaled. Domain-record
    /// equal-state branches do NOT return this — reaching them means the
    /// mutation id was never journaled, so they return `Applied` to self-heal
    /// the journal.
    Duplicate,
    /// LWW resolved against a more recent local row: incoming mutation is the
    /// documented loser, recorded in the conflict journal, never deleted.
    ConflictLoser,
    /// Permanent apply failure: quarantined in `sync_dead_letter`; cursors
    /// must not advance past this id until operator ack-skip.
    DeadLettered {
        /// Stable reason code (`malformed`, `erased_subject`, ...).
        reason_code: String,
    },
}

/// Apply context: local durable state plus this node's identity.
pub struct ApplyEngine<'a> {
    /// Local durable repository (ungated `Store` primitives).
    pub store: &'a dyn Store,
    /// Sync journal / cursors / conflicts / dead letters.
    pub sync: &'a dyn SyncStore,
    /// This node's device id (LWW tie-breaker participant).
    pub device_id: String,
    /// Local DEK provider used to seal journaled content at rest (server role).
    /// `None` marks a pure client applied-log, which journals ids/hashes only.
    pub kms: Option<&'a dyn DekProvider>,
}

/// Apply one remote mutation under the trust matrix.
///
/// Permanent rejects are quarantined (dead-letter) and reported as
/// `DeadLettered`; transient storage failures propagate as `Err` so callers
/// retry without advancing cursors.
///
/// # Errors
/// Storage failures (retryable; never advance cursors past them).
pub fn apply_mutation(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<ApplyOutcome, AmError> {
    if let Err(err) = m.validate() {
        return dead_letter(engine, m, envelope_json, &err);
    }
    if engine
        .sync
        .journal_get(&m.bank, &m.sync_mutation_id)?
        .is_some()
    {
        return Ok(ApplyOutcome::Duplicate);
    }
    // Operator-acked skips stay skipped: re-delivery of an acked poison id is
    // a documented no-op and may never re-open the row.
    if engine.sync.dead_letter_status(&m.sync_mutation_id)? == Some(DeadLetterStatus::AckedSkip) {
        return Ok(ApplyOutcome::Duplicate);
    }
    if engine.sync.journal_apply_guard_hit(
        &m.bank,
        &m.entity_kind,
        &m.entity_id,
        &m.op,
        &m.content_hash,
    )? {
        return Ok(ApplyOutcome::Duplicate);
    }
    let outcome = match (m.entity_kind.as_str(), m.op.as_str()) {
        (ENTITY_KIND_ITEM, OP_UPSERT) => apply_item_upsert(engine, m, envelope_json)?,
        (ENTITY_KIND_ITEM, OP_INVALIDATE) => apply_item_invalidate(engine, m, envelope_json)?,
        (ENTITY_KIND_ITEM, OP_DISCARD) => apply_item_discard(engine, m, envelope_json)?,
        (crate::protocol::ENTITY_KIND_ASSOC, OP_UPSERT | OP_REINFORCE) => {
            apply_assoc(engine, m, envelope_json, false)?
        }
        (crate::protocol::ENTITY_KIND_ASSOC, OP_PRUNE) => {
            apply_assoc(engine, m, envelope_json, true)?
        }
        (ENTITY_KIND_TRIPLE, OP_INVALIDATE) => apply_triple_invalidate(engine, m, envelope_json)?,
        (ENTITY_KIND_PERSONA, OP_UPSERT) => apply_records::persona_apply(engine, m, envelope_json)?,
        (ENTITY_KIND_BELIEF, OP_UPSERT) => apply_records::belief_apply(engine, m, envelope_json)?,
        (ENTITY_KIND_TASK, OP_UPSERT) => {
            crate::apply_history::task_apply(engine, m, envelope_json)?
        }
        (ENTITY_KIND_FAILURE, OP_UPSERT) => {
            crate::apply_history::failure_apply(engine, m, envelope_json)?
        }
        _ => {
            return dead_letter(
                engine,
                m,
                envelope_json,
                &AmError::not_implemented(&format!("{} / {}", m.entity_kind, m.op)),
            );
        }
    };
    if outcome == ApplyOutcome::Applied {
        let appended = engine.sync.journal_append(&build_journal_row(engine, m)?)?;
        // The journal append is the only atomic dedupe in the path (a
        // per-bank unique key inside one transaction). A concurrent apply of
        // the same mutation id can pass every check above before either of
        // them journals, so the append is what decides which one is the real
        // first apply. A push of this mutation then reports `Duplicate`
        // instead of acking a second apply that never happened.
        if !appended.inserted {
            return Ok(ApplyOutcome::Duplicate);
        }
    }
    Ok(outcome)
}

fn apply_item_upsert(
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

fn apply_item_invalidate(
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

fn apply_item_discard(
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

fn apply_assoc(
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

fn apply_triple_invalidate(
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

/// The admission score one origin receipt carries, read tolerantly: a missing
/// or non-numeric `score` yields `None`, which `lww_compare` maps to `0.0`.
/// Shared by the conflict comparison and the pull-side receipt restoration, so
/// both read the same key the same way and a malformed value degrades to the
/// documented fallback rather than to an error.
pub(crate) fn admission_score(receipt: &serde_json::Value) -> Option<f64> {
    receipt.get("score").and_then(serde_json::Value::as_f64)
}

pub(crate) fn lww_loser(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    existing_time: &str,
    existing_score: Option<f64>,
) -> Result<bool, AmError> {
    let incoming_score = m.admission_receipt.as_ref().and_then(admission_score);
    // The stored row does not carry its origin device id, so the local node's
    // device id stands in as the existing side of the tie-break. That removes
    // the old empty-string asymmetry where any remote device always won a
    // full tie. Locally held rows are the common case; a peer-origin row is
    // resolved from this node's perspective consistently.
    let incoming_wins = crate::protocol::lww_compare(
        &m.transaction_time,
        incoming_score,
        &m.device_id,
        existing_time,
        existing_score,
        &engine.device_id,
    ) == std::cmp::Ordering::Greater;
    if incoming_wins {
        return Ok(false);
    }
    engine.sync.conflict_insert(&SyncConflictRow {
        bank_id: m.bank.clone(),
        entity_kind: m.entity_kind.clone(),
        entity_id: m.entity_id.clone(),
        winner_mutation_id: format!("local:{}", m.entity_id),
        loser_mutation_id: m.sync_mutation_id.clone(),
        winner_transaction_time: existing_time.to_owned(),
        loser_transaction_time: m.transaction_time.clone(),
        winner_device_id: engine.device_id.clone(),
        loser_device_id: m.device_id.clone(),
        created_at: now_utc(),
    })?;
    Ok(true)
}

#[cfg(test)]
#[path = "apply_concurrency_tests.rs"]
mod apply_concurrency_tests;
#[cfg(test)]
#[path = "apply_matrix_tests.rs"]
mod apply_matrix_tests;
#[cfg(test)]
#[path = "apply_tests.rs"]
pub(crate) mod apply_tests;
