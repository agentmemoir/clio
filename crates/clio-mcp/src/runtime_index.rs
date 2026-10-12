// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP runtime's index-drain accessors: the
//! bounded drain entry, the post-write nudge, the background sweeper, and the
//! PII-safe pipeline snapshot.
//!
//! ## Owns
//! - `drain_index` / `nudge_index` / `spawn_index_sweeper[_with]` on `McpState`
//! - `index_status_json` and `index_status_value` (the `maintenance_status`
//!   pipeline section, from one shared drain-status read)
//!
//! ## Does not own
//! - The drain wrapper and cadence bounds (`index_drain`)
//! - `McpState` construction and field wiring (`runtime`)
//!
//! ## Boundary
//!
//! Changes to how the runtime schedules or reports outbox draining should
//! normally require changes here. Changes to outbox processing policy should not.
//!
//! Keep this module focused on runtime index-drain access.
//!
//! # Non-blocking contract
//!
//! The write path only nudges the sweeper (an O(1) flag set); embedding and
//! network I/O run on the sweeper thread. Tests and hosts call `drain_index`
//! directly to make a stored item retrievable without wall-clock timers.

use crate::index_drain::INDEX_SWEEP_INTERVAL;
use crate::maintenance_trigger::MAINTENANCE_TRIGGER_INTERVAL;
use crate::runtime::McpState;
use clio_index::{IndexStatus, ProcessReport};
use clio_types::AmError;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};

impl McpState {
    /// Drain one bounded batch of due index jobs (the testable drain entry).
    ///
    /// Never blocks a response by itself: the write path only nudges the
    /// sweeper. Tests and hosts call this to make a stored item retrievable
    /// without waiting on wall-clock timers.
    ///
    /// # Errors
    /// Queue read/write failures (per-job failures land on the job row).
    pub fn drain_index(&self) -> Result<ProcessReport, AmError> {
        self.index_drain.drain()
    }

    /// Drain the one pending index job for `(bank_id, item_id)`, if it is still
    /// queued (the one-shot write's targeted drain).
    ///
    /// A one-shot write calls this after the bounded drain so the item it just
    /// wrote is searchable even when an older backlog fills the batch.
    ///
    /// # Errors
    /// Queue read/write failures (per-job failures land on the job row).
    pub fn drain_index_item(&self, bank_id: &str, item_id: &str) -> Result<ProcessReport, AmError> {
        self.index_drain.drain_item(bank_id, item_id)
    }

    /// Nudge the background index sweeper that new outbox rows may be due.
    ///
    /// O(1) and non-blocking: it never embeds on the response path.
    /// A no-op unless [`Self::spawn_index_sweeper`] is running.
    pub fn nudge_index(&self) {
        let (lock, cvar) = &*self.index_wake;
        let mut pending = lock.lock().expect("index wake mutex poisoned");
        *pending = true;
        cvar.notify_all();
    }

    /// Start the background index sweeper with an explicit cadence.
    ///
    /// Each iteration drains one bounded batch, then waits for a nudge or the
    /// periodic timeout, whichever comes first. The periodic sweep covers
    /// retries after backoff and write paths that do not nudge. Returns the
    /// thread handle so tests can bound the wait; production callers detach it.
    #[must_use]
    pub fn spawn_index_sweeper_with(
        self: &Arc<Self>,
        interval: Duration,
    ) -> std::thread::JoinHandle<()> {
        let weak = Arc::downgrade(self);
        let wake = Arc::clone(&self.index_wake);
        let mut next_trigger_eval = Instant::now() + MAINTENANCE_TRIGGER_INTERVAL;
        std::thread::spawn(move || {
            loop {
                {
                    let (lock, cvar) = &*wake;
                    let mut pending = lock.lock().expect("index wake mutex poisoned");
                    if !*pending {
                        let (guard, _timeout) = cvar
                            .wait_timeout(pending, interval)
                            .expect("index wake mutex poisoned");
                        pending = guard;
                    }
                    *pending = false;
                }
                // Exit once the owning state is gone (tests, process shutdown).
                let Some(state) = weak.upgrade() else {
                    return;
                };
                // Provider errors may echo request text, so logs carry only stable labels.
                match state.drain_index() {
                    Err(error) => {
                        let code = error.code.as_str();
                        clio_config::log::warn("index_drain_failed", &[("error_code", code)]);
                    }
                    Ok(report) if report.failed > 0 => {
                        let failed = report.failed.to_string();
                        clio_config::log::warn(
                            "index_drain_failed",
                            &[("failed", failed.as_str()), ("error", "index_job_failed")],
                        );
                    }
                    Ok(_) => {}
                }
                // The trigger's own cadence: between drains, evaluate the
                // watched banks' maintenance pressure and fire eligible waves
                // on their own threads. The drain's cadence and the post-write
                // nudge are untouched.
                let now = Instant::now();
                if now >= next_trigger_eval {
                    next_trigger_eval = now + MAINTENANCE_TRIGGER_INTERVAL;
                    state.evaluate_maintenance_trigger(now);
                }
            }
        })
    }

    /// Start the background index sweeper at the default cadence.
    #[must_use]
    pub fn spawn_index_sweeper(self: &Arc<Self>) -> std::thread::JoinHandle<()> {
        self.spawn_index_sweeper_with(INDEX_SWEEP_INTERVAL)
    }

    /// PII-safe index pipeline snapshot for `maintenance_status`.
    ///
    /// Reports real pending/retry counts, the active space, and whether dense,
    /// rerank, and extraction are live. A defensive secret-mask pass runs over
    /// the payload before it leaves.
    ///
    /// # Errors
    /// Coverage query failures.
    pub fn index_status_json(&self, bank: &str) -> Result<Value, AmError> {
        let status = self.index_drain.status(Some(bank))?;
        Ok(self.index_status_payload(&status))
    }

    /// The same snapshot from a pre-fetched drain status.
    ///
    /// `maintenance_status` reads the drain queue once and feeds that one
    /// result here and to the pressure block, so a response can never carry
    /// two different pending counts. On a failed read the snapshot is the
    /// pressure block's unavailable marker instead of an error, so the call
    /// still returns ok; on success the shape is exactly
    /// [`Self::index_status_json`]'s payload.
    #[must_use]
    pub fn index_status_value(&self, status: &Result<IndexStatus, AmError>) -> Value {
        match status {
            Ok(status) => self.index_status_payload(status),
            Err(err) => serde_json::json!({
                "unavailable": format!("index drain status failed ({})", err.code)
            }),
        }
    }

    /// Build the masked pipeline payload from one status report.
    fn index_status_payload(&self, status: &IndexStatus) -> Value {
        let mut payload = serde_json::json!({
            "dense_available": status.embed.dense_available,
            "model_id": status.embed.model_id,
            "dims": status.embed.dims,
            "index_mode": status.embed.index_mode,
            "last_latency_ms": status.embed.last_latency_ms,
            "last_error": status.embed.last_error,
            "rerank_active": self.pipeline.rerank_active,
            "extract_available": self.pipeline.extract_available,
            "indexable_items": status.coverage.indexable_items,
            "dense_rows": status.coverage.dense_rows,
            "lexical_rows": status.coverage.lexical_rows,
            "pending_jobs": status.coverage.pending_jobs,
            "retry_jobs": status.coverage.retry_jobs,
            "coverage_last_error": status.coverage.last_error,
        });
        clio_config::secret::mask_value("index", &mut payload);
        payload
    }
}

#[cfg(test)]
#[path = "runtime_index_hardening_tests.rs"]
mod runtime_index_hardening_tests;
