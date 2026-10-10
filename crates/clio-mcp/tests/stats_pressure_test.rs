// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `stats` tool's pressure-count coverage:
//! the three relayed pressure counts must equal the values the pressure
//! block's own sources produce for the same bank (a stale stored distilled
//! unit, one hygiene archive-band item, and one hygiene flag-band item).
//!
//! ## Owns
//! - The hand-computed band fixture: hygiene weights that park a no-signal
//!   item exactly in the archive band, one flagged-band and one archive-band
//!   item, and one stored stale distilled unit
//! - The end-to-end `stats` dispatch over the MCP handler
//!
//! ## Does not own
//! - The pressure block or its sources (`clio-mcp` pressure, `clio-hygiene`)
//! - The stats report shape (`clio-compliance`)
//!
//! ## Boundary
//!
//! Tests here break when the stats report's pressure relaying or the band
//! fixtures' scoring changes; they never change production behavior.
//!
//! Keep this module focused on the stats tool's pressure counts.

use clio_config::hygiene_knobs::HygieneWeights;
use clio_mcp::protocol::McpHandler;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_store::{DistilledUnitRecord, STALE_SOURCE_CHANGED};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use clio_write::HUB_DISTILL_MARKER;
use serde_json::{Value, json};
use std::sync::Arc;

/// Bank the fixture seeds and the stats call reads.
const BANK: &str = "bank-a";
/// Fixed "now" for the stats dispatch, so band labels never depend on the clock.
const NOW: &str = "2026-10-05T00:00:00Z";
/// Created-at stamp for every fixture row; staleness weight is zero, so age
/// never moves a score.
const CREATED: &str = "2026-10-01T00:00:00Z";

/// Weights that park a no-signal item exactly in the archive band, reused from
/// the pressure block's unit fixtures: with no secret, no pattern, and no near
/// duplicate, a zero-admission item scores the utility weight alone (0.72) —
/// archive band — and a 0.2-admission item scores 0.576 — flag band.
const FIXTURE_WEIGHTS: HygieneWeights = HygieneWeights {
    secret: 0.0,
    pattern: 0.0,
    dup: 0.28,
    utility: 0.72,
    staleness: 0.0,
};

/// Distinct prose per id (no shared sketch tokens, so no near-duplicate
/// partner), reused from the pressure block's band fixtures.
fn gist(id: &str) -> &'static str {
    match id {
        "stats-archive" => "harbor mosaic tiles arranged beside the lighthouse promenade",
        "stats-flag" => "cedar rails winding across the flooded valley orchard",
        "stats-unit" => "distilled summary kept beside the stats pressure fixture",
        other => unreachable!("no fixture gist for {other}"),
    }
}

fn item(id: &str, admission: f64) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: BANK.to_owned(),
        subject_id: id.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(admission),
        source_ref: Some(format!("turn-{id}")),
        snapshot: None,
        gist: Some(gist(id).to_owned()),
        valid_from: CREATED.to_owned(),
        valid_until: None,
        tx_from: CREATED.to_owned(),
        tx_until: None,
        created_at: CREATED.to_owned(),
        updated_at: CREATED.to_owned(),
        context: None,
    }
}

/// One stored distilled unit marked stale: the reason is set, so the stale
/// count is nonzero without a distillation wave.
fn stale_unit(state: &McpState, id: &str) {
    let mut unit = item(id, 0.8);
    unit.source_ref = Some(HUB_DISTILL_MARKER.to_owned());
    let mut record = DistilledUnitRecord::new(
        BANK,
        id,
        "hub",
        Vec::<String>::new(),
        HUB_DISTILL_MARKER,
        CREATED,
    );
    record.stale_reason = Some(STALE_SOURCE_CHANGED.to_owned());
    record.stale_at = Some(CREATED.to_owned());
    state
        .store
        .commit_distilled_unit(&unit, &record, "agent")
        .expect("commit stale unit");
}

fn handler() -> Arc<McpHandler> {
    let state = McpState::open(&McpOpenOptions::default()).expect("state");
    let mut scoped = (*state).clone();
    scoped.env.hygiene = FIXTURE_WEIGHTS;
    stale_unit(&scoped, "stats-unit");
    // Zero admission with no other signal: 0.72, the archive band.
    scoped
        .store
        .create_memory_item(&item("stats-archive", 0.0), "agent")
        .expect("archive-band item");
    // 0.2 admission: 0.72 * 0.8 = 0.576, the flag band.
    scoped
        .store
        .create_memory_item(&item("stats-flag", 0.2), "agent")
        .expect("flag-band item");
    Arc::new(McpHandler::new(Arc::new(scoped)))
}

fn stats(h: &McpHandler) -> Value {
    h.handle_message(&json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "stats", "arguments": {"bank": BANK, "timestamp": NOW}}
    }))
    .expect("response")["result"]
        .clone()
}

/// The three pressure counts relay exactly what the pressure block's sources
/// produce on this fixture: one flag-band item, one stale stored unit, one
/// archive-band item — the audit difference, the stored stale reason, and the
/// bounded archive scan.
#[test]
fn stats_relays_the_pressure_counts_the_pressure_sources_produce() {
    let h = handler();
    let report = stats(&h)["structuredContent"].clone();
    assert_eq!(
        report["pressure_counts"],
        json!([
            {"label": "flagged", "count": 1},
            {"label": "stale summaries", "count": 1},
            {"label": "archive candidates", "count": 1},
        ]),
        "{report}"
    );
}
