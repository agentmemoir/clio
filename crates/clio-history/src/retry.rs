// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `prepare_retry`: the system path that surfaces
//! prior failures before an agent's next attempt at the same or a similar task.
//!
//! ## Owns
//! - The exact-then-similar retry surfacing order
//! - Retry telemetry (counts only)
//!
//! ## Does not own
//! - Similarity math (`similar`) or failure retrieval (`failure`)
//!
//! ## Boundary
//!
//! Changes to retry surfacing order or payload should normally require changes here.

use crate::failure::similar_failures;
use crate::{HistoryEvent, HistorySink, require_nonempty};
use clio_store::HistoryStore;
use clio_types::{AmError, FailureRecord, HistoryPolicy, TaskRecord};

/// Inputs for `prepare_retry`.
pub struct PrepareRetryArgs<'a> {
    /// Task history repository.
    pub history: &'a dyn HistoryStore,
    /// Bank isolation key.
    pub bank_id: &'a str,
    /// The task about to be retried (a new id is fine similar path).
    pub task_id: &'a str,
    /// Requested row count; capped by the policy.
    pub limit: Option<usize>,
    /// Similarity threshold and read bounds.
    pub policy: &'a HistoryPolicy,
    /// History telemetry sink.
    pub sink: &'a dyn HistorySink,
}

/// Result of `prepare_retry`.
#[derive(Debug, Clone, PartialEq)]
pub struct PrepareRetryOutcome {
    /// Latest task record for this id, when the caller has upserted it.
    pub task: Option<TaskRecord>,
    /// Prior failures to inject before the next attempt.
    pub failures: Vec<FailureRecord>,
    /// Task ids whose failures contributed.
    pub matched_task_ids: Vec<String>,
    /// `exact` | `similar` | `none`.
    pub mode: String,
    /// The resolved (hard-capped) row limit.
    pub limit: usize,
}

/// Surface prior failures for an upcoming attempt.
///
/// A retry is the one moment a recorded lesson can still prevent the mistake,
/// which is why this reads similar tasks and not only the exact same one.
///
/// Exact `task_id` failures win. When none exist, the task's own definition (or
/// the raw `task_id` if the task is unknown) drives similar-task matching, so a
/// retitled task with the same definition still gets the earlier lesson.
///
/// # Errors
/// Missing bank/task id, zero limit, or store failures.
pub fn prepare_retry(args: &PrepareRetryArgs<'_>) -> Result<PrepareRetryOutcome, AmError> {
    require_nonempty(args.bank_id, "bank_id")?;
    require_nonempty(args.task_id, "task_id")?;
    let limit = args.policy.resolve_limit(args.limit)?;
    let task = args.history.get_task(args.bank_id, args.task_id)?;
    let exact = args
        .history
        .failures_for_task_exact(args.bank_id, args.task_id, limit)?;
    if !exact.is_empty() {
        emit(args.sink, args, &exact, "exact");
        return Ok(PrepareRetryOutcome {
            task,
            failures: exact,
            matched_task_ids: vec![args.task_id.to_owned()],
            mode: "exact".into(),
            limit,
        });
    }
    let query_text = task
        .as_ref()
        .map_or_else(|| args.task_id.to_owned(), |t| t.definition.clone());
    let (failures, matched) =
        similar_failures(args.history, args.bank_id, &query_text, limit, args.policy)?;
    let mode = if failures.is_empty() {
        "none"
    } else {
        "similar"
    };
    emit(args.sink, args, &failures, mode);
    Ok(PrepareRetryOutcome {
        task,
        failures,
        matched_task_ids: matched,
        mode: mode.into(),
        limit,
    })
}

fn emit(
    sink: &dyn HistorySink,
    args: &PrepareRetryArgs<'_>,
    failures: &[FailureRecord],
    mode: &str,
) {
    sink.emit(HistoryEvent {
        operation: "prepare_retry".into(),
        outcome: "returned".into(),
        bank: args.bank_id.into(),
        actor: String::new(),
        timestamp: String::new(),
        task_id: args.task_id.into(),
        record_id: String::new(),
        record_count: failures.len(),
        admission_score: None,
        rejection_reason: if mode == "none" {
            Some("no prior failures".into())
        } else {
            None
        },
    });
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod retry_tests;
