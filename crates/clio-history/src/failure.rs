// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the failure tools: the gated, capped
//! `failure_record` write and the exact + similar `failures_for_task` read.
//!
//! ## Owns
//! - Lesson-cap enforcement and carrier `failure` item construction
//! - Context validation on the gated write path (fail closed before the gate)
//! - Exact-first / similar-second failure retrieval
//!
//! ## Does not own
//! - Failure persistence (`clio-store::HistoryStore`), admission math, or the
//!   retry entry point (see `retry`)
//!
//! ## Boundary
//!
//! Changes to failure write/read semantics should normally require changes here.
//! Changes to similarity math should not (`similar`).

use crate::similar::rank_similar_tasks;
use crate::{
    HistoryEvent, HistorySink, require_bank_and_timestamp, require_nonempty, unique_history_id,
};
use clio_admission::{
    AdmissionPolicy, Candidate, Decision, EventSink, GateRequest, SignalSource, gated_create,
};
use clio_store::HistoryStore;
use clio_types::{
    AmError, EpisodicType, EpistemicKind, ErrorCode, FailureRecord, HistoryPolicy, ItemKind,
    MemoryItem, OpContext, SourceType, validate_context,
};

/// Inputs for `failure_record`.
pub struct FailureRecordArgs<'a> {
    /// Task history repository.
    pub store: &'a dyn HistoryStore,
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// Data-subject id for DEK selection.
    pub subject_id: &'a str,
    /// Task this failure belongs to.
    pub task_id: &'a str,
    /// 1-based attempt number.
    pub attempt_n: i64,
    /// What went wrong.
    pub what_failed: &'a str,
    /// Length-capped lesson (rejected over the configured token cap).
    pub lesson: &'a str,
    /// Optional evidence reference.
    pub evidence_ref: Option<&'a str>,
    /// Optional bounded failure context validated, sealed into the carrier.
    pub context: Option<&'a str>,
    /// Threshold / weights for the episodic admission gate.
    pub policy: AdmissionPolicy<'a>,
    /// Lesson cap and read-bound policy.
    pub history_policy: &'a HistoryPolicy,
    /// Neighbor/recency signal source for the gate.
    pub signal_source: &'a dyn SignalSource,
    /// Admission accept/reject telemetry sink.
    pub admission_sink: &'a dyn EventSink,
    /// History telemetry sink.
    pub sink: &'a dyn HistorySink,
    /// Event timestamp.
    pub timestamp: &'a str,
    /// Score + validate without persisting (batch pre-flight mode).
    pub dry_run: bool,
}

/// Result of `failure_record`.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum FailureRecordOutcome {
    /// The record was admitted and stored.
    Stored {
        /// Persisted failure record.
        record: FailureRecord,
        /// The admission decision.
        decision: Decision,
    },
    /// Admission rejected the record; nothing was written.
    Rejected {
        /// The failing decision.
        decision: Decision,
    },
    /// Scored and staged without persisting (batch pre-flight); the record
    /// carries the admitted item id and derived score.
    DryRun {
        /// Admitted carrier item (score stamped; not persisted).
        item: MemoryItem,
        /// Would-write record (not persisted).
        record: FailureRecord,
        /// The passing admission decision.
        decision: Decision,
    },
}

/// Store a structured, gated `FailureRecord`.
///
/// A mistake that is only remembered by the person who made it is repeated, so
/// the record is structured and gated: it is either a lesson a later attempt can
/// read, or it was never stored.
///
/// The `lesson` is rejected — never truncated — when it exceeds the policy
/// token cap, so an unbounded lesson can never reach storage. `what_failed` is
/// deliberately **not** length-capped: it is the faithful factual record of the
/// failure and must not be silently truncated, while `lesson` is the
/// natural-language reinforcement surfaced into later context and therefore the
/// token-bomb surface the cap defends. Draft invariants (including
/// `attempt_n >= 1`) are checked here, before the admission gate, so a
/// malformed draft never emits an accept event. A duplicate
/// `(bank, task_id, attempt_n)` is rejected by the store's unique constraint.
///
/// # Errors
/// Missing bank/timestamp, empty required fields, `attempt_n < 1`, over-cap
/// lesson, or gate/store failures.
pub fn failure_record(args: &FailureRecordArgs<'_>) -> Result<FailureRecordOutcome, AmError> {
    let bank = require_bank_and_timestamp(args.ctx, args.timestamp)?;
    require_nonempty(args.task_id, "task_id")?;
    require_nonempty(args.what_failed, "what_failed")?;
    require_nonempty(args.subject_id, "subject_id")?;
    // Fail closed before the gate: an invalid context must never emit an
    // admission decision, let alone a write.
    if let Some(context) = args.context {
        validate_context(context)?;
    }
    if args.attempt_n < 1 {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "failure record attempt_n must be >= 1",
        ));
    }
    let lesson_tokens = args.history_policy.check_lesson(args.lesson)?;
    let item = carrier_item(args, bank, lesson_tokens);
    let req = GateRequest {
        item: &item,
        ctx: args.ctx,
        policy: args.policy.clone(),
        sink: args.admission_sink,
        timestamp: args.timestamp,
    };
    let draft = FailureRecord {
        id: unique_history_id("fr", bank, args.task_id),
        bank_id: bank.to_owned(),
        item_id: item.id.clone(),
        task_id: args.task_id.to_owned(),
        subject_id: args.subject_id.to_owned(),
        attempt_n: args.attempt_n,
        what_failed: args.what_failed.to_owned(),
        lesson: args.lesson.to_owned(),
        lesson_tokens: lesson_tokens as i64,
        evidence_ref: args.evidence_ref.map(str::to_owned),
        context: args.context.map(str::to_owned),
        admission_score: 0.0,
        epistemic_kind: EpistemicKind::Belief,
        created_at: args.timestamp.to_owned(),
    };
    let mut stored: Option<FailureRecord> = None;
    let mut carriers: Option<MemoryItem> = None;
    let decision = gated_create(
        &req,
        Candidate::Episodic(EpisodicType::Failure),
        args.signal_source,
        &mut |admitted| {
            if args.dry_run {
                let mut draft = draft.clone();
                draft.item_id.clone_from(&admitted.id);
                carriers = Some(admitted.clone());
                stored = Some(draft);
                return Ok(());
            }
            let record =
                args.store
                    .commit_failure_record(admitted, &draft, args.ctx.actor.as_str())?;
            stored = Some(record);
            Ok(())
        },
    )?;
    if !decision.pass {
        emit(
            args.sink,
            args,
            "rejected",
            "",
            Some(decision.admission_score),
            decision.rejection_reason.clone(),
        );
        return Ok(FailureRecordOutcome::Rejected { decision });
    }
    let Some(record) = stored else {
        return Err(AmError::new(
            clio_types::ErrorCode::Internal,
            "failure record stored nothing",
        ));
    };
    if args.dry_run {
        let Some(carrier) = carriers else {
            return Err(AmError::new(
                clio_types::ErrorCode::Internal,
                "failure carrier missing",
            ));
        };
        return Ok(FailureRecordOutcome::DryRun {
            item: carrier,
            record,
            decision,
        });
    }
    emit(
        args.sink,
        args,
        "stored",
        &record.id,
        Some(decision.admission_score),
        None,
    );
    Ok(FailureRecordOutcome::Stored { record, decision })
}

fn carrier_item(args: &FailureRecordArgs<'_>, bank: &str, lesson_tokens: usize) -> MemoryItem {
    MemoryItem {
        id: unique_history_id("failure-item", bank, args.task_id),
        bank_id: bank.to_owned(),
        subject_id: args.subject_id.to_owned(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Failure),
        epistemic_kind: EpistemicKind::Belief,
        source_type: Some(SourceType::AgentInferred),
        confidence: None,
        admission_score: None,
        source_ref: args.evidence_ref.map(str::to_owned),
        context: args.context.map(str::to_owned),
        snapshot: Some(serde_json::json!({
            "task_id": args.task_id,
            "attempt_n": args.attempt_n,
            "lesson_tokens": lesson_tokens,
        })),
        gist: Some(args.lesson.to_owned()),
        valid_from: args.timestamp.to_owned(),
        valid_until: None,
        tx_from: args.timestamp.to_owned(),
        tx_until: None,
        created_at: args.timestamp.to_owned(),
        updated_at: args.timestamp.to_owned(),
    }
}

fn emit(
    sink: &dyn HistorySink,
    args: &FailureRecordArgs<'_>,
    outcome: &str,
    record_id: &str,
    score: Option<f64>,
    reason: Option<String>,
) {
    sink.emit(HistoryEvent {
        operation: "failure_record".into(),
        outcome: outcome.into(),
        bank: args.ctx.bank.as_str().into(),
        actor: args.ctx.actor.as_str().into(),
        timestamp: args.timestamp.into(),
        task_id: args.task_id.into(),
        record_id: record_id.into(),
        record_count: usize::from(outcome == "stored"),
        admission_score: score,
        rejection_reason: reason,
    });
}

/// Inputs for `failures_for_task`.
pub struct FailuresForTaskArgs<'a> {
    /// Task history repository.
    pub history: &'a dyn HistoryStore,
    /// Bank isolation key.
    pub bank_id: &'a str,
    /// Exact task id or free-text task query.
    pub task_id_or_query: &'a str,
    /// Requested row count; capped by the policy.
    pub limit: Option<usize>,
    /// Similarity threshold and read bounds.
    pub policy: &'a HistoryPolicy,
    /// History telemetry sink.
    pub sink: &'a dyn HistorySink,
}

/// Result of `failures_for_task` / `prepare_retry`.
#[derive(Debug, Clone, PartialEq)]
pub struct FailureQueryOutcome {
    /// Ranked prior failures.
    pub failures: Vec<FailureRecord>,
    /// Task ids whose failures contributed.
    pub matched_task_ids: Vec<String>,
    /// `exact` | `similar` | `none`.
    pub mode: String,
    /// The resolved (hard-capped) row limit.
    pub limit: usize,
}

/// Retrieve prior failures for the same **or similar** task.
///
/// The lesson is only useful if the attempt that needs it finds it, so a
/// retitled task with the same definition still reaches its earlier failures.
///
/// Exact `task_id` matches take the primary path. When none exist and the
/// argument names a known task, that task's definition drives similar-task
/// matching; otherwise the argument itself is treated as a free-text query.
///
/// # Errors
/// Missing bank/task argument, zero limit, or store failures.
pub fn failures_for_task(args: &FailuresForTaskArgs<'_>) -> Result<FailureQueryOutcome, AmError> {
    require_nonempty(args.bank_id, "bank_id")?;
    require_nonempty(args.task_id_or_query, "task_id_or_query")?;
    let limit = args.policy.resolve_limit(args.limit)?;
    let exact = args
        .history
        .failures_for_task_exact(args.bank_id, args.task_id_or_query, limit)?;
    if !exact.is_empty() {
        emit_query(
            args.sink,
            args,
            &exact,
            &[args.task_id_or_query.to_owned()],
            "exact",
        );
        return Ok(FailureQueryOutcome {
            failures: exact,
            matched_task_ids: vec![args.task_id_or_query.to_owned()],
            mode: "exact".into(),
            limit,
        });
    }
    let query_text = match args.history.get_task(args.bank_id, args.task_id_or_query)? {
        Some(task) => task.definition,
        None => args.task_id_or_query.to_owned(),
    };
    let (failures, matched) =
        similar_failures(args.history, args.bank_id, &query_text, limit, args.policy)?;
    let mode = if failures.is_empty() {
        "none"
    } else {
        "similar"
    };
    emit_query(args.sink, args, &failures, &matched, mode);
    Ok(FailureQueryOutcome {
        failures,
        matched_task_ids: matched,
        mode: mode.into(),
        limit,
    })
}

/// Bounded similar-task failure retrieval: rank tasks by definition similarity,
/// then collect their failures until `limit` is reached.
///
/// # Errors
/// Store failures.
pub(crate) fn similar_failures(
    history: &dyn HistoryStore,
    bank_id: &str,
    query_text: &str,
    limit: usize,
    policy: &HistoryPolicy,
) -> Result<(Vec<FailureRecord>, Vec<String>), AmError> {
    let tasks = history.list_tasks(bank_id, policy.max_limit)?;
    let ranked = rank_similar_tasks(query_text, &tasks, policy);
    let mut failures: Vec<FailureRecord> = Vec::new();
    let mut matched: Vec<String> = Vec::new();
    for (_, task) in ranked {
        if failures.len() >= limit {
            break;
        }
        let remaining = limit - failures.len();
        let mut found = history.failures_for_task_exact(bank_id, &task.task_id, remaining)?;
        if !found.is_empty() {
            matched.push(task.task_id);
        }
        failures.append(&mut found);
    }
    Ok((failures, matched))
}

fn emit_query(
    sink: &dyn HistorySink,
    args: &FailuresForTaskArgs<'_>,
    failures: &[FailureRecord],
    matched: &[String],
    mode: &str,
) {
    sink.emit(HistoryEvent {
        operation: "failures_for_task".into(),
        outcome: "returned".into(),
        bank: args.bank_id.into(),
        actor: String::new(),
        timestamp: String::new(),
        task_id: matched.first().cloned().unwrap_or_default(),
        record_id: String::new(),
        record_count: failures.len(),
        admission_score: None,
        rejection_reason: if mode == "none" {
            Some("no matching failures".into())
        } else {
            None
        },
    });
}

#[cfg(test)]
#[path = "failure_tests.rs"]
mod failure_tests;

#[cfg(test)]
#[path = "failure_retrieve_isolation_tests.rs"]
mod failure_retrieve_isolation_tests;
