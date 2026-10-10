// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync apply effects of the history
//! domain records (task and failure records): payload decoding, bank/erasure
//! checks, last-writer-wins by the task version / failure attempt, and the
//! idempotent durable writes behind the `task` / `failure` apply matrix
//!
//! ## Owns
//! - Task / failure wire payload decoding (carrier item + structured record)
//! - The task apply effect (version ordering, equal-version ties, journal
//!   self-heal, carrier-context divergence detection, bank-mismatch
//!   fail-closed)
//! - The failure apply effect (`(bank, task_id, attempt_n)` slot ordering)
//! - Shared record-apply helpers `journal_conflict` and `reject`
//!
//! ## Journal self-heal
//!
//! `apply` absorbs genuine redeliveries with the pre-checked journal guards
//! (`journal_get` and the content-hash guard). Reaching an equal-state branch
//! here therefore means this mutation id was never journaled, so every
//! equal-state branch returns `Applied`: the durable record already exists
//! and the engine inserts the missing journal row, keeping the mutation
//! relayable to a third peer via pull.
//!
//! ## Does not own
//! - Item / assoc / triple apply effects (see `apply`)
//! - Persona / belief apply effects (see `apply_records`)
//! - Record persistence SQL (`clio-store` `SyncStore` history primitives)
//! - Push-side packaging of the same records (see `client_feed`)
//!
//! ## Boundary
//!
//! Changes to task/failure apply semantics should normally require changes
//! here. Changes to transport or cursor behavior should not.
//!
//! Keep this module focused on the history-record apply effects.

use crate::apply::{ApplyEngine, ApplyOutcome, dead_letter};
use crate::clock::now_utc;
use crate::protocol::{SyncContent, SyncMutation};
use clio_store::SyncConflictRow;
use clio_types::{AmError, ErrorCode, FailureRecord, MemoryItem, TaskRecord};
use serde_json::Value;

/// Deterministic conflict record: the local (stored) row wins and the incoming
/// mutation is journaled as the loser with the stored row's time as the winner.
/// Shared by every insert-only / version-ordered branch that cannot replace the
/// stored row (history records and persona/belief slots).
pub(crate) fn journal_conflict(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    stored_time: &str,
) -> Result<(), AmError> {
    engine.sync.conflict_insert(&SyncConflictRow {
        bank_id: m.bank.clone(),
        entity_kind: m.entity_kind.clone(),
        entity_id: m.entity_id.clone(),
        winner_mutation_id: format!("local:{}", m.entity_id),
        loser_mutation_id: m.sync_mutation_id.clone(),
        winner_transaction_time: stored_time.to_owned(),
        loser_transaction_time: m.transaction_time.clone(),
        winner_device_id: engine.device_id.clone(),
        loser_device_id: m.device_id.clone(),
        created_at: now_utc(),
    })
}

/// Route a payload schema/parse failure through the dead-letter quarantine: a
/// malformed domain payload is a permanent per-mutation reject, so the cursor
/// freezes for that mutation instead of the whole push/pull failing with a
/// retryable HTTP 500.
pub(crate) fn reject(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
    label: &str,
    e: &serde_json::Error,
) -> Result<ApplyOutcome, AmError> {
    dead_letter(
        engine,
        m,
        envelope_json,
        &AmError::new(ErrorCode::InvalidArgument, format!("{label}: {e}")),
    )
}

/// Apply one `task` upsert: materialize the carrier item plus a versioned
/// structured task row through the atomic history commit path.
///
/// Bank scope comes from the envelope: the payload's own carrier-item and
/// record `bank_id` fields must match it or the mutation fail-closes to the
/// dead-letter quarantine. `entity_id` must be the payload's `task_id`.
///
/// Last-writer-wins is keyed on the task version: a higher version applies,
/// an equal version is an equal-state journal self-heal (`Applied`) when the
/// record — including the carrier-bound context — matches the stored
/// row, and otherwise a journaled conflict loser; a lower version loses. An
/// incoming carrier payload without a present `context` preserves the stored
/// one (no difference), matching the preserve-on-absent rule.
pub(crate) fn task_apply(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<ApplyOutcome, AmError> {
    let SyncContent::Plain(v) = &m.content else {
        return Ok(ApplyOutcome::Applied);
    };
    let mut item: MemoryItem =
        match serde_json::from_value(v.get("item").cloned().unwrap_or(Value::Null)) {
            Ok(item) => item,
            Err(e) => return reject(engine, m, envelope_json, "bad task carrier item", &e),
        };
    let mut task: TaskRecord =
        match serde_json::from_value(v.get("task").cloned().unwrap_or(Value::Null)) {
            Ok(task) => task,
            Err(e) => return reject(engine, m, envelope_json, "bad task record", &e),
        };
    // The context rides the carrier item; a payload item without a
    // present `context` preserves the stored one (see `task_equal`).
    let incoming_has_context = matches!(
        v.get("item").and_then(|item| item.get("context")),
        Some(Value::String(_))
    );
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
    if task.bank_id != m.bank {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::Forbidden,
                "mutation bank does not match task record bank",
            ),
        );
    }
    item.bank_id.clone_from(&m.bank);
    task.bank_id.clone_from(&m.bank);
    // The store keeps the context on the carrier item (the domain reads
    // resolve it from there) and rejects a structured record that carries a
    // divergent context. Bind the record to the carrier's applied context so
    // the committed pair always matches and a task with a context applies.
    task.context.clone_from(&item.context);
    if m.entity_id != task.task_id {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::InvalidArgument,
                "task entity_id must be the task id",
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
    if let Err(err) = task.validate() {
        return dead_letter(engine, m, envelope_json, &err);
    }
    if let Err(err) = item.validate() {
        return dead_letter(engine, m, envelope_json, &err);
    }
    if let Some(current) = engine.sync.task_current(&m.bank, &task.task_id)? {
        if task.version < current.version {
            journal_conflict(engine, m, &current.created_at)?;
            return Ok(ApplyOutcome::ConflictLoser);
        }
        if task.version == current.version {
            if task_equal(&current, &task, incoming_has_context) {
                // Equal-state redelivery of an unjournaled mutation id: the
                // durable row already exists, so report Applied to let the
                // engine journal it (journal self-heal).
                return Ok(ApplyOutcome::Applied);
            }
            // Structure rows are insert-only per version: an equal-version
            // divergence keeps the local row and journals the peer as loser.
            journal_conflict(engine, m, &current.created_at)?;
            return Ok(ApplyOutcome::ConflictLoser);
        }
    }
    match engine.sync.task_put(&item, &task, "sync-apply") {
        Ok(_) => Ok(ApplyOutcome::Applied),
        // Storage failures stay retryable; validation rejects dead-letter.
        Err(err) if err.code == ErrorCode::Internal => Err(err),
        Err(err) => dead_letter(engine, m, envelope_json, &err),
    }
}

/// Apply one `failure` upsert: materialize the carrier item plus a structured
/// failure row through the atomic history commit path.
///
/// Bank scope comes from the envelope: the payload's own carrier-item and
/// record `bank_id` fields must match it or the mutation fail-closes to the
/// dead-letter quarantine. Last-writer-wins is keyed on
/// `(bank, task_id, attempt_n)`: an untouched attempt slot applies, an
/// occupied slot is an equal-state journal self-heal (`Applied`) when the
/// record id matches the stored row (journal self-heal), and otherwise
/// a journaled conflict loser.
pub(crate) fn failure_apply(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<ApplyOutcome, AmError> {
    let SyncContent::Plain(v) = &m.content else {
        return Ok(ApplyOutcome::Applied);
    };
    let mut item: MemoryItem =
        match serde_json::from_value(v.get("item").cloned().unwrap_or(Value::Null)) {
            Ok(item) => item,
            Err(e) => return reject(engine, m, envelope_json, "bad failure carrier item", &e),
        };
    let mut failure: FailureRecord =
        match serde_json::from_value(v.get("failure").cloned().unwrap_or(Value::Null)) {
            Ok(failure) => failure,
            Err(e) => return reject(engine, m, envelope_json, "bad failure record", &e),
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
    if failure.bank_id != m.bank {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::Forbidden,
                "mutation bank does not match failure record bank",
            ),
        );
    }
    item.bank_id.clone_from(&m.bank);
    failure.bank_id.clone_from(&m.bank);
    // The carrier item stores the context; bind the record to it (see
    // `task_apply`) so the committed pair always matches.
    failure.context.clone_from(&item.context);
    if m.entity_id != failure.id {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::InvalidArgument,
                "failure entity_id must be the failure id",
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
    if let Err(err) = failure.validate() {
        return dead_letter(engine, m, envelope_json, &err);
    }
    if let Err(err) = item.validate() {
        return dead_letter(engine, m, envelope_json, &err);
    }
    if let Some(current) =
        engine
            .sync
            .failure_by_attempt(&m.bank, &failure.task_id, failure.attempt_n)?
    {
        if current.id == failure.id {
            // Equal-state redelivery of an unjournaled mutation id: see the
            // journal self-heal note in `task_apply`.
            return Ok(ApplyOutcome::Applied);
        }
        journal_conflict(engine, m, &current.created_at)?;
        return Ok(ApplyOutcome::ConflictLoser);
    }
    match engine.sync.failure_put(&item, &failure, "sync-apply") {
        Ok(_) => Ok(ApplyOutcome::Applied),
        Err(err) if err.code == ErrorCode::Internal => Err(err),
        Err(err) => dead_letter(engine, m, envelope_json, &err),
    }
}

/// Exact round-trip equality for equal-version task duplicates: values ride
/// serde/SQL unchanged, so bit-identical comparison is the intended contract.
/// The context is bound to the carrier item: an incoming payload whose
/// carrier item carries no present `context` preserves the stored value (no
/// difference), while a present, differing context is an equal-version
/// divergence the caller resolves as a journaled conflict loser (never
/// silently dropped).
fn task_equal(a: &TaskRecord, b: &TaskRecord, incoming_has_context: bool) -> bool {
    let context_equal = !incoming_has_context || a.context == b.context;
    context_equal
        && a.task_id == b.task_id
        && a.definition == b.definition
        && a.steps == b.steps
        && a.status == b.status
        && a.evidence_ref == b.evidence_ref
}

#[cfg(test)]
#[path = "apply_history_edge_tests.rs"]
mod apply_history_edge_tests;
#[cfg(test)]
#[path = "apply_history_selfheal_tests.rs"]
mod apply_history_selfheal_tests;
#[cfg(test)]
#[path = "apply_history_tests.rs"]
mod apply_history_tests;
