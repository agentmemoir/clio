// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! This module is responsible for the end-to-end evidence that a read surface
//! shows a consolidated claim's freshness and that recall treats a lapsed claim
//! the way the recorded split says: `get` and `inspect` carry the block, a recall
//! hit does not, a removed source withholds its unit, and a source edit does not.
//!
//! ## Owns
//! - The read-surface and recall scenarios over a live MCP session
//!
//! ## Does not own
//! - The evidence shape, the freshness decision, and the report counters
//!   (`clio-store`, `clio-write`), and the freshness rendering (`clio-mcp`)
//! - What a wave records (`consolidated_freshness_harness`)
//! - The fixture and the readers (`consolidate_wave_support`)
//!
//! ## Boundary
//!
//! Changes to what a reader can see about a consolidated claim through the real
//! surfaces, or to what recall returns for a lapsed one, should normally require
//! changes here. Changes to what a wave records should require the other harness.
//!
//! Test-only. Never compiled into production builds.

mod common;
#[path = "consolidate_wave_support/mod.rs"]
mod consolidate_wave_support;

use consolidate_wave_support::{
    BANK, Fixture, MOVED_PAST, McpSession, claim_set, current_list, distilled_bank, entry_ids,
    evidence_of, hit_ids, suppressed_raw_ids,
};
use serde_json::{Value, json};

/// The query every recall scenario here uses: the seeded notes all share it, so a
/// scenario is about eligibility and not about which text happens to rank.
const QUERY: &str = "kubernetes staging rollout";

/// A unit that is stale for a source edit is not withheld: `consolidated_only`
/// returns it and `prefer_consolidated` does not suppress the raw item it may no
/// longer represent. This is the recorded split, over the real tool.
///
/// The suppression half needed the recorded decision to be implemented: a wave
/// writes `source_changed` onto the record it already re-reads, and that write
/// re-projects the item's snapshot, which is the only place the retrieve path
/// looks. So a wave stands in for the reason, and the two calls below run after
/// one — the window between an edit and a wave is the accepted cost of keeping
/// recall snapshot-only, and it is `get`/`inspect` that see the lapse in it.
#[test]
fn a_source_changed_unit_is_returned_by_both_scopes() {
    let (f, unit) = distilled_bank();
    f.keep_only_unit(&unit);
    let (_, _, source) = claim_set(&f, &unit);
    f.set_item_updated_at(&source, MOVED_PAST);
    // The wave that records the reason. `--force` runs the reconcile and the
    // freshness pass over the one unit the fixture keeps.
    let waved = f.json(&["consolidate", "--force", "--output", "json"]);
    assert_eq!(waved["ok"], json!(true), "{waved}");
    assert_eq!(
        f.stale_reason_of(&unit),
        "source_changed",
        "the wave wrote the reason recall will read"
    );

    let mut session = McpSession::open(&f);
    let only = session.call(
        2,
        "retrieve",
        json!({"query": QUERY, "recall_scope": "consolidated_only", "limit": 20}),
    );
    let returned = hit_ids(&only);
    assert!(
        returned.contains(&unit),
        "a lapsed-but-not-removed unit is still returned by the consolidated \
         scope: {only}"
    );
    assert_eq!(
        only["dedup"]["scope_empty"],
        json!(false),
        "and the scope is not empty: {only}"
    );
    for hit in only["hits"].as_array().expect("hits") {
        assert_eq!(
            hit["consolidated"],
            json!(true),
            "consolidated_only returned something that is not a unit: {hit}"
        );
        assert!(
            hit.get("freshness").is_none(),
            "and no hit carries a freshness field: {hit}"
        );
    }

    let prefer = session.call(
        3,
        "retrieve",
        json!({"query": QUERY, "prefer_consolidated": true, "limit": 20}),
    );
    let suppressed = suppressed_raw_ids(&prefer);
    assert!(
        !suppressed.contains(&source),
        "a parent that no longer represents this source must not suppress it: {prefer}"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );
}

/// A unit whose source was withdrawn is withheld from `consolidated_only`, while
/// its raw source stays visible: the other half of the recorded split, over the
/// real tool, in the same shape as the changed-unit case above.
#[test]
fn a_source_removed_unit_is_withheld_and_its_raw_source_stays_visible() {
    let (f, unit) = distilled_bank();
    f.keep_only_unit(&unit);
    let (_, _, source) = claim_set(&f, &unit);
    f.assert_ok(&[
        "discard",
        &source,
        "--reason",
        "withdrawn by the operator",
        "--confirm",
        "--output",
        "json",
    ]);

    let mut session = McpSession::open(&f);
    let only = session.call(
        2,
        "retrieve",
        json!({"query": QUERY, "recall_scope": "consolidated_only", "limit": 20}),
    );
    assert!(
        !hit_ids(&only).contains(&unit),
        "a removed source's unit is not current for the consolidated scope: {only}"
    );
    assert_eq!(
        only["dedup"]["scope_empty"],
        json!(true),
        "and the scope reports itself empty rather than widening: {only}"
    );

    let full = session.call(
        3,
        "retrieve",
        json!({"query": QUERY, "prefer_consolidated": true, "limit": 20}),
    );
    let returned = hit_ids(&full);
    assert!(
        !returned.contains(&unit) || !returned.contains(&source),
        "a withheld unit does not sit on the page suppressing its own source: {full}"
    );
    let suppressed = suppressed_raw_ids(&full);
    assert!(
        suppressed.is_empty() || !suppressed.contains(&source),
        "and nothing suppresses the withdrawn source: {full}"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );
}

/// `get` and `inspect` both carry the block for a current unit, and a recall hit
/// carries no freshness field at all: the hit contract is unchanged.
#[test]
fn the_read_surfaces_carry_the_block_and_a_recall_hit_does_not() {
    let (f, unit) = distilled_bank();
    // One consolidated item, so the recall page below is about the unit under
    // test rather than about which of eighteen summaries happens to rank.
    f.keep_only_unit(&unit);
    let mut session = McpSession::open(&f);
    let got = session.call(2, "get", json!({"item_id": unit}));
    let block = &got["freshness"];
    let expected = entry_ids(&current_list(&evidence_of(&f, &unit)));
    assert_eq!(block["stale"], json!(false), "{got}");
    assert_eq!(
        block["proof_count"].as_u64(),
        Some(expected.len() as u64),
        "the count is the current revision's distinct sources: {got}"
    );
    assert_eq!(block["stale_reason"], Value::Null, "{got}");
    assert!(
        block["last_reconciled_at"].is_string(),
        "the reconciliation time is reported: {got}"
    );

    let listed = session.call(3, "inspect", json!({"limit": 50}));
    let rows = listed["items"].as_array().expect("inspect rows");
    let row = rows
        .iter()
        .find(|r| r["item_id"] == json!(unit.as_str()))
        .unwrap_or_else(|| panic!("the unit is not listed: {listed}"));
    assert_eq!(row["freshness"]["stale"], json!(false), "{row}");
    assert_eq!(
        row["freshness"]["proof_count"], block["proof_count"],
        "both surfaces report the same count: {row}"
    );

    let recalled = session.call(4, "retrieve", json!({"query": QUERY, "limit": 20}));
    let hits = recalled["hits"].as_array().expect("recall hits");
    let hit = hits
        .iter()
        .find(|h| h["item_id"] == json!(unit.as_str()))
        .unwrap_or_else(|| panic!("the unit was not recalled: {recalled}"));
    assert!(
        hit.get("freshness").is_none(),
        "the recall hit shape is unchanged: {hit}"
    );
    for key in hit.as_object().expect("a hit is an object").keys() {
        assert!(
            !key.contains("stale") && !key.contains("proof") && !key.contains("reconcil"),
            "no freshness field on a hit: {key}"
        );
    }
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );
}

/// A read of something that is not a distilled unit is unchanged: no key is
/// added for an ordinary item, which is what keeps the block additive.
#[test]
fn a_read_of_an_ordinary_item_gains_no_freshness_key() {
    let f = Fixture::new();
    f.assert_ok(&[
        "remember",
        "an ordinary note about the payment schedule",
        "--category",
        "schema",
        "--actor",
        "agent",
        "--output",
        "json",
    ]);
    let id = f
        .stored_item_ids()
        .into_iter()
        .next()
        .expect("the note was stored");
    let got = f.json(&["get", &id, "--output", "json"]);
    assert_eq!(got["ok"], json!(true), "{got}");
    assert!(
        got.get("freshness").is_none(),
        "an ordinary item gains no key: {got}"
    );
    let listed = f.json(&["inspect", "--limit", "10", "--output", "json"]);
    for row in listed["items"].as_array().expect("rows") {
        assert!(
            row.get("freshness").is_none(),
            "and no inspect row does either: {row}"
        );
    }
    let _ = BANK;
}
