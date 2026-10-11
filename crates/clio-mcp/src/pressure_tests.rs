// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `pressure` block's unit tests: the
//! hand-computed component counts, the absent-engine reason, the archive band
//! count (archive band counted, discard band excluded), the scan budget's
//! incomplete flag, the block's merge into `maintenance_status`, and the
//! published description's budget and `timestamp` contract. Source-failure
//! tests live in `pressure_failing_store_tests`.
//!
//! ## Owns
//! - Test-only. Never compiled into production builds.
//!
//! ## Does not own
//! - The pressure block itself (`pressure`)
//! - The stores and engines it reads (`clio-store`, `clio-write`)
//!
//! ## Boundary
//!
//! Tests here break when the pressure block or its component shapes change;
//! they never change production behavior.
//!
//! Keep this module focused on the pressure block's read-only contract.

use crate::runtime::{McpOpenOptions, McpState};
use clio_config::hygiene_knobs::HygieneWeights;
use clio_config::retention::ResolvedRetention;
use clio_store::{AssocPolicy, DistilledUnitRecord, STALE_SOURCE_CHANGED};
use clio_types::{Actor, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory};
use clio_write::{HUB_DISTILL_MARKER, HubDistillEngine, MaintenanceStatusData};
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) const BANK: &str = "bank-a";
pub(crate) const NOW: &str = "2026-10-05T00:00:00Z";
const SECRET: &str = "api_key: sk-live-abcdefgh1234";

/// An archive-band fixture item: zero admission, distinct prose per id (no
/// shared sketch tokens, so no near-duplicate partner), no secret or pattern
/// signal — the score is the utility weight alone (0.72).
fn archive_band_item(id: &str) -> MemoryItem {
    let gist = match id {
        "noise-archive" => "quokka ledger notes kept for the archive band fixture",
        "band-archive" => "harbor mosaic tiles arranged beside the lighthouse promenade",
        "band-1" => "cedar rails winding across the flooded valley orchard",
        "band-2" => "midnight printers humming inside the annexe corridor",
        other => unreachable!("no archive-band gist for {other}"),
    };
    item(id, gist, 0.0, "2026-10-01T00:00:00Z")
}

/// Weights that park a no-signal item exactly in the archive band: with no
/// secret, no pattern, and no near duplicate, a zero-admission item scores
/// the utility weight alone (0.72) — above the 0.70 archive floor and below
/// the 0.90 discard floor, whatever its age.
const FIXTURE_WEIGHTS: HygieneWeights = HygieneWeights {
    secret: 0.0,
    pattern: 0.0,
    dup: 0.28,
    utility: 0.72,
    staleness: 0.0,
};

pub(crate) fn state() -> Arc<McpState> {
    state_for_bank("default")
}

pub(crate) fn state_for_bank(bank: &str) -> Arc<McpState> {
    let options = McpOpenOptions {
        default_bank: bank.to_owned(),
        ..McpOpenOptions::default()
    };
    McpState::open_with_effective(&options, &serde_json::json!({})).expect("state")
}

fn item(id: &str, gist: &str, admission: f64, created_at: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: id.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(admission),
        source_ref: Some(format!("turn-{id}")),
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: created_at.into(),
        valid_until: None,
        tx_from: created_at.into(),
        tx_until: None,
        created_at: created_at.into(),
        updated_at: created_at.into(),
        context: None,
    }
}

/// The handler's own status result, built by hand: the pressure block must
/// count exactly the ancestors it is given, never consult the forest.
pub(crate) fn status_with(dirty: usize, refreshing: usize) -> MaintenanceStatusData {
    MaintenanceStatusData {
        leaf_queryable: true,
        structural_maintenance_ms: 0,
        time_to_queryable_ms: 0,
        dirty_ancestors: (0..dirty).map(|i| format!("dirty-{i}")).collect(),
        refreshing: (0..refreshing).map(|i| format!("refresh-{i}")).collect(),
        last_error: None,
        item_id: None,
    }
}

/// Commit one distilled unit; `is_stale` marks it with a stored stale reason.
pub(crate) fn commit_unit(state: &McpState, id: &str, is_stale: bool) {
    let mut unit = item(id, "distilled summary", 0.8, "2026-10-01T00:00:00Z");
    unit.source_ref = Some(HUB_DISTILL_MARKER.to_owned());
    let mut record = DistilledUnitRecord::new(
        BANK,
        id,
        "hub",
        Vec::<String>::new(),
        HUB_DISTILL_MARKER,
        "2026-10-01T00:00:00Z",
    );
    if is_stale {
        record.stale_reason = Some(STALE_SOURCE_CHANGED.to_owned());
        record.stale_at = Some("2026-10-02T00:00:00Z".to_owned());
    }
    state
        .store
        .commit_distilled_unit(&unit, &record, "agent")
        .expect("commit unit");
}

/// One hub with degree 2 against a limit of 1, on a clock pinned at millis 0,
/// plus a finished wave on the empty bank. A wave that reconciles nothing
/// records no tally on a fresh bank, so `last_wave` stays `null` — a known,
/// deterministic value under the record-survival semantics.
fn scoped_state(s: &Arc<McpState>) -> Arc<HubDistillEngine> {
    let engine = Arc::new(HubDistillEngine::with_clock(
        s.store.clone(),
        s.env.clone(),
        AssocPolicy {
            hub_degree_limit: 1,
            ..AssocPolicy::default()
        },
        0,
    ));
    let ctx = OpContext::new(Bank::new(BANK), Actor::parse("agent").expect("actor"));
    engine
        .consolidate_hubs(BANK, &ctx, &ResolvedRetention::default())
        .expect("empty wave");
    s.store
        .reinforce_assoc_edge(BANK, "hub", "n1", 0, 1.0)
        .expect("edge");
    s.store
        .reinforce_assoc_edge(BANK, "hub", "n2", 0, 1.0)
        .expect("edge");
    engine
}

#[test]
fn component_counts_match_a_hand_computed_fixture() {
    let s = state();
    let engine = scoped_state(&s);
    let mut scoped = (*s).clone();
    scoped.hub = Some(engine);
    scoped.env.hygiene = FIXTURE_WEIGHTS;
    // Two distilled units, one stale (the only stored stale reason).
    commit_unit(&s, "unit-current", false);
    commit_unit(&s, "unit-stale", true);
    // The credential leak is forced to the hygiene discard band (>= 0.90), so
    // it is NOT an archive candidate; the zero-admission item with no other
    // signal scores 0.72 and IS one.
    s.store
        .create_memory_item(
            &item("noise-secret", SECRET, 0.5, "2026-10-01T00:00:00Z"),
            "agent",
        )
        .expect("item");
    s.store
        .create_memory_item(&archive_band_item("noise-archive"), "agent")
        .expect("item");
    s.store
        .create_memory_item(
            &item(
                "noise-clean",
                "service listens on port 8080",
                0.9,
                "2026-10-04T00:00:00Z",
            ),
            "agent",
        )
        .expect("item");
    // Clear the index rows the five creates enqueued, then queue exactly
    // three jobs and move two of them into retry.
    for id in [
        "unit-current",
        "unit-stale",
        "noise-secret",
        "noise-archive",
        "noise-clean",
    ] {
        s.index.finish_index_job(BANK, id).expect("finish job");
    }
    for id in ["job-1", "job-2", "job-3"] {
        s.store
            .create_memory_item(
                &item(id, "plain note", 0.5, "2026-10-04T00:00:00Z"),
                "agent",
            )
            .expect("item");
    }
    s.index
        .fail_index_job(BANK, "job-1", "synthetic failure")
        .expect("retry");
    s.index
        .fail_index_job(BANK, "job-2", "synthetic failure")
        .expect("retry");

    let drain = scoped.index_drain.status(Some(BANK));
    let out = super::block(&scoped, BANK, &status_with(3, 1), NOW, &drain);
    assert_eq!(
        out,
        json!({
            "dirty_ancestors": {"count": 3},
            "refreshing_ancestors": {"count": 1},
            "eligible_hubs": {"count": 1},
            "pending_jobs": {"count": 3},
            "retry_jobs": {"count": 2},
            "stale_units": {
                "count": 1,
                "source": "stored_state",
                "last_wave": null
            },
            "archive_candidates": {"count": 1, "incomplete": false}
        }),
        "{out}"
    );
}

#[test]
fn absent_hub_engine_is_unavailable_and_others_still_report() {
    let s = state();
    let mut scoped = (*s).clone();
    scoped.hub = None;
    let drain = scoped.index_drain.status(Some(BANK));

    let out = super::block(&scoped, BANK, &status_with(2, 0), NOW, &drain);
    let reason = out["eligible_hubs"]["unavailable"]
        .as_str()
        .expect("unavailable reason");
    assert!(
        reason.contains("hub distillation engine is not configured"),
        "{reason}"
    );
    assert!(reason.contains("factor_unavailable"), "{reason}");
    assert!(out["eligible_hubs"].get("count").is_none(), "{out}");
    assert_eq!(out["dirty_ancestors"], json!({"count": 2}), "{out}");
    assert_eq!(out["refreshing_ancestors"], json!({"count": 0}), "{out}");
    assert_eq!(out["pending_jobs"], json!({"count": 0}), "{out}");
    assert_eq!(out["retry_jobs"], json!({"count": 0}), "{out}");
    assert_eq!(out["stale_units"]["count"], json!(0), "{out}");
    assert_eq!(out["stale_units"]["source"], json!("stored_state"), "{out}");
    assert_eq!(out["stale_units"]["last_wave"], Value::Null, "{out}");
    assert_eq!(
        out["archive_candidates"],
        json!({"count": 0, "incomplete": false}),
        "{out}"
    );
}

#[test]
fn archive_band_counts_band_items_and_excludes_discard_band_items() {
    let s = state();
    let mut scoped = (*s).clone();
    scoped.env.hygiene = FIXTURE_WEIGHTS;
    // The credential leak: a secret hit forces the score to the discard band.
    s.store
        .create_memory_item(
            &item("band-secret", SECRET, 0.5, "2026-10-01T00:00:00Z"),
            "agent",
        )
        .expect("item");
    // The archive-band item: no secret, no pattern, no near duplicate, zero
    // admission — the utility weight alone (0.72) puts it in 0.70–0.89.
    s.store
        .create_memory_item(&archive_band_item("band-archive"), "agent")
        .expect("item");
    // Prove both items really sit in the claimed bands before counting them.
    let report = clio_hygiene::hygiene_audit(&clio_hygiene::AuditArgs {
        store: &*s.store,
        bank: BANK,
        now: NOW,
        min_score: super::ARCHIVE_BAND_MIN_SCORE,
        limit: 200,
        offset: 0,
        weights: &scoped.env.hygiene,
        stale_half_life_days: scoped.env.hygiene_stale_half_life_days,
        max_scan: clio_hygiene::DEFAULT_MAX_SCAN,
    })
    .expect("audit");
    let archive = report
        .candidates
        .iter()
        .find(|c| c.item_id == "band-archive")
        .expect("archive candidate");
    assert_eq!(
        archive.suggested_action.as_deref(),
        Some("archive"),
        "{archive:?}"
    );
    assert!(
        (super::ARCHIVE_BAND_MIN_SCORE..super::DISCARD_BAND_MIN_SCORE)
            .contains(&archive.noise_score),
        "{}",
        archive.noise_score
    );
    let secret = report
        .candidates
        .iter()
        .find(|c| c.item_id == "band-secret")
        .expect("secret candidate");
    assert_eq!(
        secret.suggested_action.as_deref(),
        Some("discard"),
        "{secret:?}"
    );
    assert!(
        secret.noise_score >= super::DISCARD_BAND_MIN_SCORE,
        "{}",
        secret.noise_score
    );
    // Only the archive band counts; the discard-band secret hit does not.
    let count = super::archive_candidates(&scoped, BANK, NOW, super::ARCHIVE_SCAN_BUDGET);
    assert_eq!(count, json!({"count": 1, "incomplete": false}), "{count}");
}

#[test]
fn archive_count_carries_the_incomplete_flag() {
    let s = state();
    let mut scoped = (*s).clone();
    scoped.env.hygiene = FIXTURE_WEIGHTS;
    for id in ["band-1", "band-2"] {
        s.store
            .create_memory_item(&archive_band_item(id), "agent")
            .expect("item");
    }
    let full = super::archive_candidates(&scoped, BANK, NOW, super::ARCHIVE_SCAN_BUDGET);
    assert_eq!(full, json!({"count": 2, "incomplete": false}), "{full}");
    // A scan budget of one scores one item: the count stays a partial count
    // and the audit's own flag says so.
    let partial = super::archive_candidates(&scoped, BANK, NOW, 1);
    assert_eq!(
        partial,
        json!({"count": 1, "incomplete": true}),
        "{partial}"
    );
}

#[test]
fn maintenance_status_merges_pressure_without_touching_existing_fields() {
    let s = state_for_bank(BANK);
    let out = crate::read_tools::dispatch(
        &s,
        "maintenance_status",
        &json!({"bank": BANK, "timestamp": NOW}),
    )
    .expect("dispatch");
    // The pre-existing fields keep their real values on this fixture.
    assert_eq!(out["leaf_queryable"], json!(true), "{out}");
    assert!(out["structural_maintenance_ms"].is_u64(), "{out}");
    assert!(out["time_to_queryable_ms"].is_u64(), "{out}");
    assert_eq!(out["dirty_ancestors"], json!([]), "{out}");
    assert_eq!(out["refreshing"], json!([]), "{out}");
    // None-valued fields are skipped by the serializer, so absent is correct.
    assert!(out.get("last_error").is_none(), "{out}");
    assert!(out.get("item_id").is_none(), "{out}");
    // The index block is the unchanged payload, byte for byte.
    assert_eq!(
        out["index"],
        s.index_status_json(BANK).expect("json"),
        "{out}"
    );
    assert!(out["index"]["pending_jobs"].is_u64(), "{out}");
    let pressure = out["pressure"].as_object().expect("pressure object");
    let mut keys: Vec<&str> = pressure.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "archive_candidates",
            "dirty_ancestors",
            "eligible_hubs",
            "pending_jobs",
            "refreshing_ancestors",
            "retry_jobs",
            "stale_units"
        ]
    );
    for (name, entry) in pressure {
        assert!(
            entry.get("count").is_some() || entry.get("unavailable").is_some(),
            "{name}: {entry}"
        );
    }
    // The trigger block sits next to the pressure block; on this fresh state
    // it renders the explicit never-evaluated marker.
    assert_eq!(
        out["trigger"],
        json!({
            "evaluated": false,
            "unavailable": "trigger has not evaluated this bank",
        }),
        "{out}"
    );
}

#[test]
fn the_published_description_matches_the_bounded_archive_scan() {
    let defs = crate::schema_read_defs::read_defs();
    let def = defs
        .iter()
        .find(|def| def.name == "maintenance_status")
        .expect("maintenance_status def");
    // The published budget matches the production constant, so the two
    // cannot drift apart silently.
    assert!(
        def.description
            .contains(&format!("at most {}", super::ARCHIVE_SCAN_BUDGET)),
        "{}",
        def.description
    );
    // The schema declares the timestamp argument the handler honours.
    assert!(
        def.props.iter().any(|(key, _)| *key == "timestamp"),
        "{:?}",
        def.props
    );
    // The published row names the trigger block and its never-evaluated
    // marker, so the block's absence of a bank is never read as silence.
    assert!(
        def.description.contains("`trigger` block")
            && def
                .description
                .contains("has not evaluated renders an explicit unavailable marker"),
        "{}",
        def.description
    );
}
