// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This crate is responsible for the history tool surface: task history,
//! length-capped failure lessons, retry surfacing, and the
//! `temporal_history` read façade.
//!
//! ## Owns
//! - `task_upsert` / `task_get` / `task_history`
//! - `failure_record` / `failures_for_task` / `prepare_retry`
//! - `temporal_history` target dispatch and lexical similar-task matching
//! - PII-safe history telemetry (ids + counts only)
//!
//! ## Does not own
//! - Persistence (`clio-store::HistoryStore`), scoring math (`clio-admission`), or
//!   hybrid retrieve (`clio-retrieve`); history carrier items are written here so
//!   the existing domain filters can select them.
//!
//! ## Boundary
//!
//! Changes to history tool semantics or similarity thresholds should normally
//! require changes here. Changes to history SQL or admission math should not.
//!
//! # Bounded reads
//!
//! No history tool replays a full transcript. Every read resolves a hard row
//! limit (`HistoryPolicy`) and the similar-task path scans only the bounded
//! `list_tasks`/`list_failures` window. There is deliberately no "list all
//! messages" or full-log API.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::float_cmp))]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

pub mod failure;
pub mod retry;
pub mod similar;
pub mod task;
pub mod temporal;

pub use failure::{
    FailureQueryOutcome, FailureRecordArgs, FailureRecordOutcome, FailuresForTaskArgs,
    failure_record, failures_for_task,
};
pub use retry::{PrepareRetryArgs, PrepareRetryOutcome, prepare_retry};
pub use similar::{RETITLE_FIXTURES, lexical_similarity, rank_similar_tasks};
pub use task::{
    TaskGetArgs, TaskGetOutcome, TaskHistoryArgs, TaskHistoryOutcome, TaskUpsertArgs,
    TaskUpsertOutcome, task_get, task_history, task_upsert,
};
pub use temporal::{
    TemporalHistoryArgs, TemporalHistoryOutcome, TemporalTargetKind, temporal_history,
};

pub use clio_store::HistoryStore;
pub use clio_types::{
    DEFAULT_HISTORY_LIMIT, DEFAULT_LESSON_MAX_TOKENS, DEFAULT_TASK_SIMILARITY_THRESHOLD,
    FailureRecord, HistoryPolicy, MAX_HISTORY_LIMIT, TaskRecord,
};

/// PII-safe telemetry for history writes and reads (ids + counts only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEvent {
    /// `task_upsert` | `failure_record` | `failures_for_task` | `prepare_retry` | `temporal_history`.
    pub operation: String,
    /// `stored` | `rejected` | `returned`.
    pub outcome: String,
    /// Bank isolation key.
    pub bank: String,
    /// Attribution actor (empty on reads).
    pub actor: String,
    /// Caller timestamp.
    pub timestamp: String,
    /// Task identifier involved (empty when not applicable).
    pub task_id: String,
    /// Stored/returned record id (empty for multi-row reads).
    pub record_id: String,
    /// Number of records returned or written.
    pub record_count: usize,
    /// Admission score when one was stamped or considered.
    pub admission_score: Option<f64>,
    /// Reject reason when `outcome = rejected`.
    pub rejection_reason: Option<String>,
}

/// Sink for history telemetry.
pub trait HistorySink: std::fmt::Debug {
    /// Record one event.
    fn emit(&self, event: HistoryEvent);
}

/// In-memory sink for tests and single-process hosts.
#[derive(Debug, Default)]
pub struct HistoryVecSink {
    events: Mutex<Vec<HistoryEvent>>,
}

impl HistoryVecSink {
    /// Snapshot of recorded events.
    ///
    /// # Panics
    /// Only when the sink mutex is poisoned (a bug in this process).
    #[must_use]
    pub fn events(&self) -> Vec<HistoryEvent> {
        self.events.lock().expect("history sink poisoned").clone()
    }
}

impl HistorySink for HistoryVecSink {
    fn emit(&self, event: HistoryEvent) {
        self.events
            .lock()
            .expect("history sink poisoned")
            .push(event);
    }
}

/// A no-op sink for hosts that do not record history telemetry.
#[derive(Debug, Default)]
pub struct NoopHistorySink;

impl HistorySink for NoopHistorySink {
    fn emit(&self, _event: HistoryEvent) {}
}

/// Reject empty required strings on the history tool surface.
pub(crate) fn require_nonempty(value: &str, label: &str) -> Result<(), clio_types::AmError> {
    if value.is_empty() {
        return Err(clio_types::AmError::new(
            clio_types::ErrorCode::InvalidArgument,
            format!("{label} is required"),
        ));
    }
    Ok(())
}

/// Reject an empty bank or timestamp and return the bank string.
pub(crate) fn require_bank_and_timestamp<'a>(
    ctx: &'a clio_types::OpContext,
    timestamp: &str,
) -> Result<&'a str, clio_types::AmError> {
    if ctx.bank.as_str().is_empty() {
        return Err(clio_types::AmError::new(
            clio_types::ErrorCode::MissingBank,
            "bank is required",
        ));
    }
    require_nonempty(timestamp, "timestamp")?;
    Ok(ctx.bank.as_str())
}

/// Collision-resistant history row id (per-process monotonic suffix).
pub(crate) fn unique_history_id(prefix: &str, bank_id: &str, key: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{prefix}-{bank_id}-{key}-{nanos}-{seq}")
}

#[cfg(test)]
mod lib_tests;
#[cfg(test)]
mod test_support;
