// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for timing the operator-visible pressure read and
//! each of its component sources over one fixture, and checking one generous
//! p95 ceiling per measurement.
//!
//! ## Owns
//! - The timing method: discarded warm-ups, timed samples, min/median/p95/max
//! - The per-backend `Bounds` and the `MEASURE` report lines
//! - The block-shape, component-count, bank-count, and archive-count
//!   assertions taken before timing
//!
//! ## Does not own
//! - The fixture contents (`fixture`)
//! - The trigger measurements (`measure_trigger`)
//! - The pressure block or its component sources (the runtime crates)
//!
//! ## Boundary
//!
//! Changes to the timing method or the ceilings should normally require changes
//! here. Changes to what the fixture seeds should not require changes here.
//!
//! Keep this module focused on the pressure-read measurements.
//
// Method, fixed for repeat runs: build the fixture once, take 50 timed samples
// per measurement after discarding 10 warm-up calls, wrap every timed result
// call in `std::hint::black_box`, then print min/median/p95/max and assert only
// a generous p95 ceiling. The sample count stays small because the nextest
// measurement override gives these two tests a 120 s slow period and terminates
// them after three slow periods, so a clean run must stay well inside that
// 360 s cap. `std::time::Instant` is monotonic. The ceilings reject an
// order-of-magnitude regression only: this sandbox runner is shared with other
// work, so a loaded machine is not the timing verdict and a tight per-call or
// mean assertion is never used.

use crate::fixture::{Fixture, MAIN_BANK, NOW};
use clio_mcp::runtime::McpState;
use clio_mcp::{
    ARCHIVE_BAND_MIN_SCORE, ARCHIVE_PAGE_SIZE, ARCHIVE_SCAN_BUDGET, DISCARD_BAND_MIN_SCORE,
};
use clio_types::ToolResult;
use serde_json::json;
use std::hint::black_box;
use std::time::Instant;

/// Discarded warm-up calls before sampling.
const WARMUP: usize = 10;
/// Timed samples per measurement: 10 warm-ups plus 50 samples keeps a clean
/// run well inside the measurement override's 360 s cap.
pub(crate) const SAMPLES: usize = 50;
/// The seven pressure components, sorted for a deterministic comparison.
const PRESSURE_KEYS: [&str; 7] = [
    "archive_candidates",
    "dirty_ancestors",
    "eligible_hubs",
    "pending_jobs",
    "refreshing_ancestors",
    "retry_jobs",
    "stale_units",
];

/// Latency summary of one measurement, in milliseconds.
#[derive(Clone, Copy)]
pub struct Stats {
    /// Smallest sample.
    pub min: f64,
    /// Middle sample after sorting.
    pub median: f64,
    /// Sample at the 95th percentile index.
    pub p95: f64,
    /// Largest sample.
    pub max: f64,
}

/// Generous p95 ceilings, in milliseconds, one per measurement.
pub struct Bounds {
    /// Full `maintenance_status` dispatch (the operator-visible read).
    pub full_read: f64,
    /// `clio_write::maintenance_status` (dirty and refreshing ancestors).
    pub maintenance_status: f64,
    /// `HubDistillEngine::eligible_hubs`.
    pub eligible_hubs: f64,
    /// `IndexDrain::status` (pending and retry jobs).
    pub index_drain: f64,
    /// `Store::list_distilled_units` (stale units).
    pub distilled_units: f64,
    /// `clio_hygiene::hygiene_audit` at the archive band.
    pub hygiene_audit: f64,
}

impl Bounds {
    /// SQLite in-memory ceilings, set from a clean 50-sample run: they bound
    /// the measured archive scan, which scores at most `ARCHIVE_SCAN_BUDGET`
    /// (256) rows per band scan, with margin; the small sources sit at the
    /// 50 ms floor.
    #[must_use]
    pub fn sqlite() -> Self {
        Self {
            full_read: 2000.0,
            maintenance_status: 50.0,
            eligible_hubs: 50.0,
            index_drain: 100.0,
            distilled_units: 50.0,
            hygiene_audit: 2000.0,
        }
    }

    /// Postgres loopback ceilings, set from a clean 50-sample run: they bound
    /// the measured archive scan, which scores at most `ARCHIVE_SCAN_BUDGET`
    /// (256) rows per band scan, with margin; the small sources sit at the
    /// 50 ms floor.
    #[must_use]
    pub fn postgres() -> Self {
        Self {
            full_read: 4000.0,
            maintenance_status: 50.0,
            eligible_hubs: 50.0,
            index_drain: 100.0,
            distilled_units: 50.0,
            hygiene_audit: 4000.0,
        }
    }
}

/// Time `op`: discard warm-ups, take samples, return the sorted summary.
pub(crate) fn time<F: FnMut() -> u64>(mut op: F) -> Stats {
    for _ in 0..WARMUP {
        black_box(op());
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        black_box(op());
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
    }
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    Stats {
        min: samples[0],
        median: samples[n / 2],
        p95: samples[n * 95 / 100],
        max: samples[n - 1],
    }
}

/// Print one `MEASURE` line and assert the generous ceiling for it.
pub(crate) fn report(backend: &str, fx: &Fixture, component: &str, stats: &Stats, bound_ms: f64) {
    eprintln!(
        "MEASURE backend={backend} banks={} items={} samples={SAMPLES} \
         component={component} min_ms={:.3} median_ms={:.3} p95_ms={:.3} \
         max_ms={:.3} bound_ms={bound_ms:.3} machine=idle-required",
        fx.banks.len(),
        fx.item_count,
        stats.min,
        stats.median,
        stats.p95,
        stats.max,
    );
    assert_bound(backend, fx, component, stats, bound_ms);
}

/// Assert the generous ceiling: it rejects an order-of-magnitude regression
/// only, and it holds only on an idle machine.
pub(crate) fn assert_bound(
    backend: &str,
    fx: &Fixture,
    component: &str,
    stats: &Stats,
    bound_ms: f64,
) {
    assert!(
        stats.p95 <= bound_ms,
        "{backend} {component}: p95 {:.3} ms exceeds the generous bound {bound_ms:.3} ms \
         over {} banks and {} stored items; the bound holds only on an idle machine, and \
         a shared, possibly loaded runner can raise samples, so re-run once on an idle \
         machine before calling this a regression",
        stats.p95,
        fx.banks.len(),
        fx.item_count,
    );
}

/// Assert the fixture exercises the block, then measure all seven components.
pub fn run(backend: &str, fx: &Fixture, bounds: &Bounds) {
    let state: &McpState = &fx.state;
    let args = json!({"bank": MAIN_BANK, "timestamp": NOW});

    assert_block_shape(state, fx, &args);
    measure_read_paths(backend, fx, state, &args, bounds);
    measure_sources(backend, fx, state, bounds);
}

/// Check the block shape and the fixture's component counts on the same state
/// the timed read uses, so timing cannot hide a broken payload.
fn assert_block_shape(state: &McpState, fx: &Fixture, args: &serde_json::Value) {
    let out = clio_mcp::write_tools::dispatch(state, "maintenance_status", args);
    assert_eq!(out["ok"], json!(true), "{out}");
    let pressure = out["pressure"].as_object().expect("pressure block object");
    let mut keys: Vec<&str> = pressure.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, PRESSURE_KEYS, "{out}");
    for (name, entry) in pressure {
        assert!(
            entry.get("count").is_some() || entry.get("unavailable").is_some(),
            "{name}: {entry}"
        );
    }
    for component in PRESSURE_KEYS {
        let entry = &out["pressure"][component];
        assert!(
            entry["count"].as_u64().unwrap_or(0) > 0,
            "fixture does not exercise {component}: {entry}"
        );
    }
    let bank_ids = state.ops_store.bank_ids().expect("fixture bank list");
    for bank in &fx.banks {
        assert!(
            bank_ids.contains(bank),
            "bank list {bank_ids:?} misses fixture bank {bank}"
        );
    }
    assert_eq!(
        bank_ids.len(),
        fx.banks.len(),
        "the store reports {} banks but the fixture created {}: the per-bank and \
         trigger-projection figures divide and multiply by the fixture count, so an \
         extra or missing bank would make both wrong: {bank_ids:?}",
        bank_ids.len(),
        fx.banks.len(),
    );
    // The block's archive count must equal a recomputation from the block's
    // own re-exported constants this measurement times, so block/measurement
    // drift fails here instead of timing a scan the block no longer runs.
    let block_count = out["pressure"]["archive_candidates"]["count"]
        .as_u64()
        .expect("archive_candidates count");
    assert_eq!(
        block_count,
        recomputed_archive_count(state),
        "the block reports {block_count} archive candidates but the measurement's audit \
         at min_score {ARCHIVE_BAND_MIN_SCORE}, page {ARCHIVE_PAGE_SIZE}, and \
         budget {ARCHIVE_SCAN_BUDGET} computes a different count: the block and \
         the measurement have drifted apart",
    );
}

/// Time the operator-visible read against its ceiling.
fn measure_read_paths(
    backend: &str,
    fx: &Fixture,
    state: &McpState,
    args: &serde_json::Value,
    bounds: &Bounds,
) {
    // The operator-visible read: the whole dispatch, pressure block included.
    let full = time(|| {
        let out = clio_mcp::write_tools::dispatch(state, "maintenance_status", args);
        out["pressure"]["archive_candidates"]["count"]
            .as_u64()
            .unwrap_or(u64::MAX)
    });
    report(
        backend,
        fx,
        "maintenance_status_dispatch",
        &full,
        bounds.full_read,
    );
}

/// Time each source API once, through its own public entry point.
fn measure_sources(backend: &str, fx: &Fixture, state: &McpState, bounds: &Bounds) {
    measure_maintenance_status(backend, fx, state, bounds);
    measure_eligible_hubs(backend, fx, state, bounds);
    measure_index_drain(backend, fx, state, bounds);
    measure_distilled_units(backend, fx, state, bounds);
    measure_archive_candidates(backend, fx, state, bounds);
}

/// Dirty and refreshing ancestors through `clio_write::maintenance_status`.
fn measure_maintenance_status(backend: &str, fx: &Fixture, state: &McpState, bounds: &Bounds) {
    let maint = time(
        || match clio_write::maintenance_status(&state.maint, MAIN_BANK, None) {
            ToolResult::Ok { data, .. } => u64::try_from(
                data.dirty_ancestors
                    .len()
                    .saturating_add(data.refreshing.len()),
            )
            .unwrap_or(u64::MAX),
            ToolResult::Err { message, .. } => {
                panic!("maintenance_status source failed: {message}")
            }
        },
    );
    report(
        backend,
        fx,
        "clio_write_maintenance_status",
        &maint,
        bounds.maintenance_status,
    );
}

/// Eligible hubs: the read-only degree scan over one bank's edges.
fn measure_eligible_hubs(backend: &str, fx: &Fixture, state: &McpState, bounds: &Bounds) {
    let hubs = time(|| {
        let engine = state.hub.as_ref().expect("fixture hub engine");
        u64::try_from(engine.eligible_hubs(MAIN_BANK).expect("hub scan").len()).unwrap_or(u64::MAX)
    });
    report(
        backend,
        fx,
        "hub_eligible_hubs",
        &hubs,
        bounds.eligible_hubs,
    );
}

/// Pending and retry jobs: one bank-scoped index-drain status.
fn measure_index_drain(backend: &str, fx: &Fixture, state: &McpState, bounds: &Bounds) {
    let drain = time(|| {
        let status = state
            .index_drain
            .status(Some(MAIN_BANK))
            .expect("index drain status");
        status
            .coverage
            .pending_jobs
            .saturating_add(status.coverage.retry_jobs)
    });
    report(
        backend,
        fx,
        "index_drain_status",
        &drain,
        bounds.index_drain,
    );
}

/// Stale units: the stored distilled-unit list for one bank.
fn measure_distilled_units(backend: &str, fx: &Fixture, state: &McpState, bounds: &Bounds) {
    let units = time(|| {
        u64::try_from(
            state
                .store
                .list_distilled_units(MAIN_BANK)
                .expect("distilled units")
                .len(),
        )
        .unwrap_or(u64::MAX)
    });
    report(
        backend,
        fx,
        "stored_distilled_units",
        &units,
        bounds.distilled_units,
    );
}

/// Archive candidates: the hygiene audit with the same band, page, and scan
/// budget the pressure block uses.
fn measure_archive_candidates(backend: &str, fx: &Fixture, state: &McpState, bounds: &Bounds) {
    let audit = time(|| {
        let audit = clio_hygiene::hygiene_audit(&archive_audit_args(state, ARCHIVE_BAND_MIN_SCORE))
            .expect("archive scan");
        u64::try_from(audit.total_candidates).unwrap_or(u64::MAX)
    });
    report(
        backend,
        fx,
        "hygiene_audit_archive_band",
        &audit,
        bounds.hygiene_audit,
    );
}

/// The audit arguments the block's archive scan uses, at `min_score`.
fn archive_audit_args(state: &McpState, min_score: f64) -> clio_hygiene::AuditArgs<'_> {
    clio_hygiene::AuditArgs {
        store: &*state.store,
        bank: MAIN_BANK,
        now: NOW,
        min_score,
        limit: ARCHIVE_PAGE_SIZE,
        offset: 0,
        weights: &state.env.hygiene,
        stale_half_life_days: state.env.hygiene_stale_half_life_days,
        max_scan: ARCHIVE_SCAN_BUDGET,
    }
}

/// The archive count the block should report, recomputed with the block's
/// re-exported constants: the archive-band audit minus the discard-band audit,
/// exactly as the block computes it.
fn recomputed_archive_count(state: &McpState) -> u64 {
    let with_discard =
        clio_hygiene::hygiene_audit(&archive_audit_args(state, ARCHIVE_BAND_MIN_SCORE))
            .expect("archive-band audit");
    let discard_only =
        clio_hygiene::hygiene_audit(&archive_audit_args(state, DISCARD_BAND_MIN_SCORE))
            .expect("discard-band audit");
    u64::try_from(
        with_discard
            .total_candidates
            .saturating_sub(discard_only.total_candidates),
    )
    .expect("archive count fits in u64")
}
