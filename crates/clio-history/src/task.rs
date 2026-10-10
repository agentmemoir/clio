// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the task tools: gated `task_upsert`,
//! `task_get`, and bounded `task_history`.
//!
//! ## Owns
//! - Carrier `task` item construction and the gated write call
//! - Context validation on the gated write path (fail closed before the gate)
//! - Ancestor-gist lookup (optional MemTree link) and bounded history reads
//!
//! ## Does not own
//! - Task persistence (`clio-store::HistoryStore`) or admission math
//!
//! ## Boundary
//!
//! Changes to task tool semantics should normally require changes here.
//! Changes to task storage should not require changes here.

use crate::{
    HistoryEvent, HistorySink, require_bank_and_timestamp, require_nonempty, unique_history_id,
};
use clio_admission::{
    AdmissionPolicy, Candidate, Decision, EventSink, GateRequest, SignalSource, gated_create,
};
use clio_store::{HistoryStore, MemTreeQuery, Store};
use clio_types::{
    AmError, EpisodicType, EpistemicKind, HistoryPolicy, ItemKind, MemoryItem, OpContext,
    TaskRecord, validate_context,
};

/// Inputs for `task_upsert`.
pub struct TaskUpsertArgs<'a> {
    /// Task history repository.
    pub store: &'a dyn HistoryStore,
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// Data-subject id for DEK selection.
    pub subject_id: &'a str,
    /// Caller task identifier.
    pub task_id: &'a str,
    /// Task definition (required).
    pub definition: &'a str,
    /// Ordered steps taken so far.
    pub steps: &'a [String],
    /// Free-form status label.
    pub status: &'a str,
    /// Optional evidence reference.
    pub evidence_ref: Option<&'a str>,
    /// Optional bounded task context validated, sealed into the carrier.
    pub context: Option<&'a str>,
    /// Threshold / weights for the episodic admission gate.
    pub policy: AdmissionPolicy<'a>,
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

/// Result of `task_upsert`.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum TaskUpsertOutcome {
    /// The version was admitted and stored.
    Stored {
        /// Persisted task record (with derived version).
        record: TaskRecord,
        /// The admission decision.
        decision: Decision,
    },
    /// Admission rejected the version; nothing was written.
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
        record: TaskRecord,
        /// The passing admission decision.
        decision: Decision,
    },
}

/// Create a new gated task-history version.
///
/// The version number is derived inside the store transaction. Concurrent
/// upserts on the same `(bank, task_id)` may collide on the unique version; the
/// store returns a structured conflict error on the loser.
///
/// # Errors
/// Missing bank/timestamp, empty definition/id, or gate/store failures.
pub fn task_upsert(args: &TaskUpsertArgs<'_>) -> Result<TaskUpsertOutcome, AmError> {
    let bank = require_bank_and_timestamp(args.ctx, args.timestamp)?;
    require_nonempty(args.task_id, "task_id")?;
    require_nonempty(args.definition, "definition")?;
    require_nonempty(args.status, "status")?;
    require_nonempty(args.subject_id, "subject_id")?;
    // Fail closed before the gate: an invalid context must never emit an
    // admission decision, let alone a write.
    if let Some(context) = args.context {
        validate_context(context)?;
    }
    let item = carrier_item(args, bank);
    let req = GateRequest {
        item: &item,
        ctx: args.ctx,
        policy: args.policy.clone(),
        sink: args.admission_sink,
        timestamp: args.timestamp,
    };
    let draft = TaskRecord {
        id: unique_history_id("tr", bank, args.task_id),
        bank_id: bank.to_owned(),
        item_id: item.id.clone(),
        task_id: args.task_id.to_owned(),
        subject_id: args.subject_id.to_owned(),
        version: 1,
        definition: args.definition.to_owned(),
        steps: args.steps.to_vec(),
        status: args.status.to_owned(),
        evidence_ref: args.evidence_ref.map(str::to_owned),
        context: args.context.map(str::to_owned),
        admission_score: 0.0,
        created_at: args.timestamp.to_owned(),
    };
    let mut stored: Option<TaskRecord> = None;
    let mut carriers: Option<MemoryItem> = None;
    let decision = gated_create(
        &req,
        Candidate::Episodic(EpisodicType::Task),
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
                    .commit_task_upsert(admitted, &draft, args.ctx.actor.as_str())?;
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
            0,
            Some(decision.admission_score),
            decision.rejection_reason.clone(),
        );
        return Ok(TaskUpsertOutcome::Rejected { decision });
    }
    let Some(record) = stored else {
        return Err(AmError::new(
            clio_types::ErrorCode::Internal,
            "task upsert stored nothing",
        ));
    };
    if args.dry_run {
        let Some(carrier) = carriers else {
            return Err(AmError::new(
                clio_types::ErrorCode::Internal,
                "task carrier missing",
            ));
        };
        return Ok(TaskUpsertOutcome::DryRun {
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
        1,
        Some(decision.admission_score),
        None,
    );
    Ok(TaskUpsertOutcome::Stored { record, decision })
}

fn carrier_item(args: &TaskUpsertArgs<'_>, bank: &str) -> MemoryItem {
    let item_id = unique_history_id("task-item", bank, args.task_id);
    MemoryItem {
        id: item_id,
        bank_id: bank.to_owned(),
        subject_id: args.subject_id.to_owned(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: args.evidence_ref.map(str::to_owned),
        context: args.context.map(str::to_owned),
        snapshot: Some(serde_json::json!({
            "task_id": args.task_id,
            "status": args.status,
            "steps": args.steps,
        })),
        gist: Some(args.definition.to_owned()),
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
    args: &TaskUpsertArgs<'_>,
    outcome: &str,
    record_id: &str,
    count: usize,
    score: Option<f64>,
    reason: Option<String>,
) {
    sink.emit(HistoryEvent {
        operation: "task_upsert".into(),
        outcome: outcome.into(),
        bank: args.ctx.bank.as_str().into(),
        actor: args.ctx.actor.as_str().into(),
        timestamp: args.timestamp.into(),
        task_id: args.task_id.into(),
        record_id: record_id.into(),
        record_count: count,
        admission_score: score,
        rejection_reason: reason,
    });
}

/// Inputs for `task_get`.
pub struct TaskGetArgs<'a> {
    /// Task history repository.
    pub history: &'a dyn HistoryStore,
    /// Item store used for the optional MemTree ancestor lookup.
    pub store: &'a dyn Store,
    /// Bank isolation key.
    pub bank_id: &'a str,
    /// Caller task identifier.
    pub task_id: &'a str,
}

/// Result of `task_get`.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskGetOutcome {
    /// Latest task version, or `None` when the task is unknown in this bank.
    pub record: Option<TaskRecord>,
    /// Summarized ancestor gist when MemTree nodes are linked to this task.
    pub ancestor_gist: Option<String>,
}

/// Fetch the latest task version plus an optional summarized ancestor.
///
/// # Errors
/// Missing bank/task id or store failures.
pub fn task_get(args: &TaskGetArgs<'_>) -> Result<TaskGetOutcome, AmError> {
    require_nonempty(args.bank_id, "bank_id")?;
    require_nonempty(args.task_id, "task_id")?;
    let record = args.history.get_task(args.bank_id, args.task_id)?;
    let query = MemTreeQuery {
        bank_id: args.bank_id.to_owned(),
        task_id: Some(args.task_id.to_owned()),
        time_start: None,
        time_end: None,
        depth: None,
        limit: Some(64),
    };
    let nodes = args.store.query_memtree_nodes(&query)?;
    let ancestor_gist = nodes
        .into_iter()
        .filter(|n| n.depth > 0 && n.summary_gist.is_some())
        .max_by_key(|n| n.depth)
        .and_then(|n| n.summary_gist);
    Ok(TaskGetOutcome {
        record,
        ancestor_gist,
    })
}

/// Inputs for `task_history`.
pub struct TaskHistoryArgs<'a> {
    /// Task history repository.
    pub history: &'a dyn HistoryStore,
    /// Bank isolation key.
    pub bank_id: &'a str,
    /// Caller task identifier.
    pub task_id: &'a str,
    /// Requested row count; capped by the policy.
    pub limit: Option<usize>,
    /// Bounded-read policy.
    pub policy: &'a HistoryPolicy,
}

/// Result of `task_history`.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskHistoryOutcome {
    /// Task versions, newest first.
    pub records: Vec<TaskRecord>,
    /// The resolved (hard-capped) row limit.
    pub limit: usize,
}

/// Prior task versions, newest first, hard-capped.
///
/// # Errors
/// Missing bank/task id, zero limit, or store failures.
pub fn task_history(args: &TaskHistoryArgs<'_>) -> Result<TaskHistoryOutcome, AmError> {
    require_nonempty(args.bank_id, "bank_id")?;
    require_nonempty(args.task_id, "task_id")?;
    let limit = args.policy.resolve_limit(args.limit)?;
    let records = args
        .history
        .task_history(args.bank_id, args.task_id, limit)?;
    Ok(TaskHistoryOutcome { records, limit })
}

#[cfg(test)]
#[path = "task_tests.rs"]
mod task_tests;

#[cfg(test)]
#[path = "task_domain_tests.rs"]
mod task_domain_tests;

#[cfg(test)]
#[path = "task_postgres_parity_tests.rs"]
mod task_postgres_parity_tests;
