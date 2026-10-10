// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for assembling the `maintenance_status` pressure
//! block: read-only counts of the maintenance work a bank is carrying, and the
//! `trigger` block: the automatic wave trigger's last recorded decision.
//!
//! ## Owns
//! - The `pressure` JSON shape: one entry per maintenance component
//! - Mapping each component's source to `{"count"}` or a static `{"unavailable"}`
//! - The archive-candidate count: the hygiene score bands' `archive` band
//!   (0.70 to below 0.90, discard band excluded), from a bounded on-demand
//!   scan, with its published budget and band constants
//! - The `trigger` JSON shape: the trigger's snapshot rendered for humans,
//!   with the never-evaluated state rendered explicitly
//!
//! ## Does not own
//! - Maintenance state itself (`clio-write`, the hub engine, the index drain)
//! - Hygiene scoring or the audit scan (`clio-hygiene`)
//! - Tool routing or the surrounding status payload (`read_tools`)
//!
//! ## Boundary
//!
//! Changes to the pressure components or their shapes should normally require
//! changes here. Changes to the stores or engines this reads should not require
//! changes here.
//!
//! Keep this module focused on counting pressure. It never starts work, never
//! locks the forest, never writes, and never fails the whole status call.

use crate::maintenance_trigger::{DeclineReason, MaintenanceTriggerStatus, PressureCounts};
use crate::runtime::McpState;
use clio_index::IndexStatus;
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::time::Instant;

/// Archive band floor: a scanned item at or above this score sits in the
/// hygiene `archive` band. Mirrors the action bands in `clio-hygiene`'s
/// scoring (`suggested_action`, scoring.rs), which publishes no constant.
pub const ARCHIVE_BAND_MIN_SCORE: f64 = 0.70;

/// Discard band floor: at or above it (or on a secret hit) the hygiene bands
/// say `discard`, never `archive`. Mirrors the same `suggested_action` bands
/// in `clio-hygiene`'s scoring.rs; the count subtracts this band so a
/// discard-band item is never reported as an archive candidate.
pub const DISCARD_BAND_MIN_SCORE: f64 = 0.90;

/// Candidate page size for the archive scan: the audit's `limit` parameter,
/// which caps how many ranked candidates the page carries. The block reports
/// only the total count, so the page stays minimal; the work bound is
/// [`ARCHIVE_SCAN_BUDGET`], not this value.
pub const ARCHIVE_PAGE_SIZE: usize = 1;

/// Scan budget for the pressure block's archive scan: at most this many
/// active items are scored per band scan.
///
/// `maintenance_status` runs on every dispatch, and one scored item costs a
/// full store read plus the audit's token sketch and near-duplicate work. The
/// archive count is a report, not a cleanup pass, so 256 items bounds that
/// cost far below the hygiene tool's own 10,000-item default while still
/// answering small banks exactly; a larger bank answers a partial count
/// flagged `incomplete` instead of a slow exact one.
pub const ARCHIVE_SCAN_BUDGET: usize = 256;

/// Assemble the read-only pressure block for one bank.
///
/// Every component answers with `{"count": n}` or, when its source is absent
/// or fails, with `{"unavailable": "<static reason> (<error code>)"}`. A
/// failing component never propagates an error and never fabricates a zero.
/// `drain` is the one drain-status read the handler already made: the index
/// block and this block must report the same queue state.
#[must_use]
pub(crate) fn block(
    state: &McpState,
    bank: &str,
    status: &clio_write::MaintenanceStatusData,
    now: &str,
    drain: &Result<IndexStatus, AmError>,
) -> Value {
    let (pending_jobs, retry_jobs) = index_jobs(drain);
    json!({
        // Source: the `clio_write::maintenance_status` result the handler
        // already computed — never a second call and never the forest lock.
        "dirty_ancestors": count_entry(status.dirty_ancestors.len() as u64),
        "refreshing_ancestors": count_entry(status.refreshing.len() as u64),
        // Source: the hub engine's eligibility scan, when one is wired.
        "eligible_hubs": eligible_hubs(state, bank),
        // Source: the handler's single index drain status read, shared with
        // the index block.
        "pending_jobs": pending_jobs,
        "retry_jobs": retry_jobs,
        // Source: stored distilled-unit records, plus the last finished wave's
        // snapshot from the hub engine when one ran in this process.
        "stale_units": stale_units(state, bank),
        // Source: the hygiene score bands' archive band, scanned on demand.
        "archive_candidates": archive_candidates(state, bank, now, ARCHIVE_SCAN_BUDGET),
    })
}

/// A countable component: one `count` key.
fn count_entry(n: u64) -> Value {
    json!({ "count": n })
}

/// An unavailable component: a static reason plus the source's error code.
///
/// Never carries the source's raw message, ids, or content.
fn unavailable_entry(reason: &str, code: ErrorCode) -> Value {
    json!({ "unavailable": format!("{reason} ({code})") })
}

/// Eligible hub count from the hub engine.
///
/// Source: `HubDistillEngine::eligible_hubs(bank)`, a read-only degree scan.
/// No engine wired is unavailable, never a fabricated zero.
fn eligible_hubs(state: &McpState, bank: &str) -> Value {
    match state.hub.as_ref() {
        Some(engine) => match engine.eligible_hubs(bank) {
            Ok(hubs) => count_entry(hubs.len() as u64),
            Err(err) => unavailable_entry("hub eligibility scan failed", err.code),
        },
        None => unavailable_entry(
            "hub distillation engine is not configured",
            ErrorCode::FactorUnavailable,
        ),
    }
}

/// Pending and retried index jobs from the handler's one drain status read.
///
/// Source: `IndexDrain::status(Some(bank))` fields `pending_jobs` /
/// `retry_jobs` (whole queue depth and the failed-at-least-once subset).
fn index_jobs(drain: &Result<IndexStatus, AmError>) -> (Value, Value) {
    match drain {
        Ok(status) => (
            count_entry(status.coverage.pending_jobs),
            count_entry(status.coverage.retry_jobs),
        ),
        Err(err) => {
            let entry = unavailable_entry("index drain status failed", err.code);
            (entry.clone(), entry)
        }
    }
}

/// Stale distilled-unit count and the last wave that recorded one.
///
/// Source: `Store::list_distilled_units(bank)`, counting records whose
/// `stale_reason` is set; the wave side is
/// `HubDistillEngine::last_wave_stale(bank)`. The wave is process-local, so
/// `null` means no productive wave ran here, not that the bank is current.
pub(crate) fn stale_units(state: &McpState, bank: &str) -> Value {
    let units = match state.store.list_distilled_units(bank) {
        Ok(units) => units,
        Err(err) => {
            // The attempted source stays named when it fails, so the key is
            // stable across the success and failure shapes.
            let mut entry = unavailable_entry("stored stale state read failed", err.code);
            entry["source"] = json!("stored_state");
            return entry;
        }
    };
    let stale_count = units
        .iter()
        .filter(|unit| unit.stale_reason.is_some())
        .count();
    let last_wave = state
        .hub
        .as_ref()
        .and_then(|engine| engine.last_wave_stale(bank))
        .map_or(Value::Null, |(tally, at_millis)| {
            json!({
                "at_millis": at_millis,
                "tally": serde_json::to_value(tally).expect("serialize stale tally"),
            })
        });
    json!({
        "count": stale_count as u64,
        "source": "stored_state",
        "last_wave": last_wave,
    })
}

/// Archive-candidate count and whether the scan budget cut the scan short.
///
/// Source: `clio_hygiene::hygiene_audit`, run twice over the same bounded row
/// prefix: once at [`ARCHIVE_BAND_MIN_SCORE`] (every archive- or discard-band
/// item) and once at [`DISCARD_BAND_MIN_SCORE`] (discard-band only). The
/// count is the difference, so a secret-hit item (its score is forced to the
/// discard floor) is never counted, and the band edges are the two mirrored
/// floors rather than a re-derived formula. `incomplete` is the archive-band
/// scan's own flag; both scans read the same rows under the same budget, so
/// they agree on it. `max_scan` is a parameter so a test can reach the
/// partial path on a small fixture; [`block`] passes [`ARCHIVE_SCAN_BUDGET`].
pub(crate) fn archive_candidates(
    state: &McpState,
    bank: &str,
    now: &str,
    max_scan: usize,
) -> Value {
    let args = |min_score: f64| clio_hygiene::AuditArgs {
        store: &*state.store,
        bank,
        now,
        min_score,
        limit: ARCHIVE_PAGE_SIZE,
        offset: 0,
        weights: &state.env.hygiene,
        stale_half_life_days: state.env.hygiene_stale_half_life_days,
        max_scan,
    };
    let with_discard = match clio_hygiene::hygiene_audit(&args(ARCHIVE_BAND_MIN_SCORE)) {
        Ok(report) => report,
        Err(err) => return unavailable_entry("hygiene archive scan failed", err.code),
    };
    let discard_only = match clio_hygiene::hygiene_audit(&args(DISCARD_BAND_MIN_SCORE)) {
        Ok(report) => report,
        Err(err) => return unavailable_entry("hygiene archive scan failed", err.code),
    };
    json!({
        "count": with_discard
            .total_candidates
            .saturating_sub(discard_only.total_candidates),
        "incomplete": with_discard.incomplete,
    })
}

/// Assemble the `trigger` block for one bank: the automatic maintenance
/// trigger's last recorded decision, read from the trigger's own snapshot.
///
/// Source: `MaintenanceTriggerState::status(bank)`, the per-bank record the
/// sweeper's evaluation writes. This block reports that record and never
/// recomputes the counts, so it can never disagree with the evaluation that
/// made the decision. `now` is the render instant, a parameter so tests
/// inject instants instead of reading the clock. `std::time::Instant` carries
/// no wall-clock time, so the wave instants render as elapsed seconds
/// relative to `now` — the same honesty rule as the pressure block's
/// process-local `last_wave` stamp — and are never fabricated into
/// wall-clock times. A bank the trigger never evaluated renders an explicit
/// unavailable marker, never a silent absence of the block.
#[must_use]
pub(crate) fn trigger_block(status: Option<MaintenanceTriggerStatus>, now: Instant) -> Value {
    let Some(status) = status else {
        return json!({
            "evaluated": false,
            "unavailable": "trigger has not evaluated this bank",
        });
    };
    json!({
        // The last evaluation's outcome, so the trigger is explainable: what
        // it saw, whether it fired, and why it did not.
        "evaluated": true,
        "fired": status.fired,
        "last_decline": status.last_decline.map(DeclineReason::as_str),
        "waves_last_hour": status.waves_last_hour,
        // The pressure read the evaluation decided on, verbatim from the
        // snapshot; `null` is unreachable after an evaluation but rendered
        // honestly rather than fabricated into a zero.
        "pressure": status.pressure.map(trigger_pressure_value),
        // Elapsed-relative renderings of the monotonic wave instants. An
        // instant the clock has not yet reached renders `null` rather than a
        // negative count; an already-passed next-eligible time renders `0`.
        "last_wave_secs_ago": status
            .last_wave
            .and_then(|wave| now.checked_duration_since(wave))
            .map(|since| since.as_secs()),
        "next_eligible_in_secs": status
            .next_eligible
            .map(|eligible| eligible.saturating_duration_since(now).as_secs()),
    })
}

/// The three triggering counts, verbatim from the evaluation's pressure read.
fn trigger_pressure_value(counts: PressureCounts) -> Value {
    json!({
        "dirty_ancestors": counts.dirty_ancestors,
        "eligible_hubs": counts.eligible_hubs,
        "stale_units": counts.stale_units,
    })
}

#[cfg(test)]
#[path = "pressure_tests.rs"]
mod pressure_tests;

#[cfg(test)]
#[path = "pressure_failing_store_tests.rs"]
mod pressure_failing_store_tests;

#[cfg(test)]
#[path = "pressure_trigger_tests.rs"]
mod pressure_trigger_tests;
