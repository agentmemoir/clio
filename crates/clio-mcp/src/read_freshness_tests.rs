// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-surface freshness tests: the block a
//! distilled unit's `get` and `inspect` carry, the record resolution behind it,
//! the honest rendering of current, stale, and unknown, and the guard that the
//! block widens nothing.
//!
//! ## Owns
//! - `get`/`inspect` freshness block scenarios and their distilled-unit fixtures
//!
//! ## Does not own
//! - The freshness decision itself (`clio-store`'s `distilled_freshness` tests)
//! - The generic read matrix (`read_tools_tests`) and recall scope/dedup
//!   (`read_dedup_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_store::{DistilledUnitRecord, evidence_from_items, snapshot_projection};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::sync::Arc;

/// The instant revision 1 reconciles at. Every stamp in these fixtures is
/// written out rather than read from the host clock: freshness compares
/// store-stored stamps, so a fixture stamped "now" would read as current on one
/// machine and stale on another.
const RECONCILED_AT: &str = "2026-01-02T00:00:00Z";
/// The instant revision 2 reconciles at, after `RECONCILED_AT`.
const REVISED_AT: &str = "2026-01-03T00:00:00Z";
/// A source last changed before both reconciliations, so a unit built from it
/// reads as current at either revision.
const SOURCE_AS_OF: &str = "2026-01-01T00:00:00Z";
/// A source the write path never stamped: an absent stamp is visible, not
/// invented.
const UNSTAMPED: &str = "";
/// The unit's own prose, which must never reach the freshness block.
const UNIT_GIST: &str = "the summarized cluster in prose";

fn state() -> Arc<McpState> {
    // Hermetic: `open` layers the machine's deployment file, which turns these
    // into timing flakes where sidecars live. Same fixture as `read_dedup_tests`.
    McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: "bank-a".into(),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// A semantic item stamped at `updated_at`, so the claim-state read freshness
/// depends on is a fact of the fixture rather than of when the test ran.
fn semantic_item(id: &str, bank: &str, gist: &str, updated_at: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: Some("turn-1".into()),
        snapshot: Some(json!({ "entity": gist })),
        gist: Some(gist.into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: updated_at.into(),
        context: None,
    }
}

/// Write one item plus its lexical doc, as `read_dedup_tests` does.
fn put(state: &McpState, item: &MemoryItem, text: &str) {
    state
        .store
        .create_memory_item(item, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(&item.bank_id, &item.id, text)
        .expect("lexical doc");
}

/// Commit one distilled unit with its derivation record, exactly as a hub
/// distillation write does, and hand the record back for assertions.
///
/// The record is built through the store's own two rules — `evidence_from_items`
/// for the list and `record_evidence` for its placement — so the fixture is the
/// shape a real write produces rather than a hand-rolled one.
fn distill(
    state: &McpState,
    unit_id: &str,
    claimed: &[&MemoryItem],
    reconciled_at: &str,
) -> DistilledUnitRecord {
    let (hub, sources) = claimed.split_first().expect("a unit claims a hub");
    let mut record = DistilledUnitRecord::new(
        "bank-a",
        unit_id,
        &hub.id,
        sources.iter().map(|i| i.id.clone()),
        clio_retrieve::HUB_DISTILL_SOURCE_REF,
        reconciled_at,
    );
    record.record_evidence(evidence_from_items(claimed.iter().copied()), reconciled_at);
    state
        .store
        .commit_distilled_unit(&unit_item(&record), &record, "tester")
        .expect("distill");
    record
}

/// The unit's item: the record's own marker and the record's own snapshot
/// projection, which is what a distillation write puts on the item.
fn unit_item(record: &DistilledUnitRecord) -> MemoryItem {
    let mut item = semantic_item(
        &record.current_revision_item_id(),
        &record.bank_id,
        UNIT_GIST,
        SOURCE_AS_OF,
    );
    item.source_ref = Some(record.marker.clone());
    item.snapshot = Some(snapshot_projection(record));
    item
}

/// A value's keys, sorted, so two responses' shapes can be compared without
/// depending on the order either one happened to serialize in.
fn keys_of(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("object response")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// One bank holding a hub, one source, and one item that is not distilled.
fn hub_bank(state: &McpState) -> (MemoryItem, MemoryItem, MemoryItem) {
    let hub = semantic_item("HUB-1", "bank-a", "alpha hub note", SOURCE_AS_OF);
    let source = semantic_item("SRC-1", "bank-a", "alpha source note", SOURCE_AS_OF);
    let plain = semantic_item("PLAIN-1", "bank-a", "alpha plain note", SOURCE_AS_OF);
    put(state, &hub, "alpha hub note");
    put(state, &source, "alpha source note");
    put(state, &plain, "alpha plain note");
    (hub, source, plain)
}

#[test]
fn get_on_a_current_distilled_unit_reports_a_freshness_block() {
    let s = state();
    let (hub, source, _) = hub_bank(&s);
    distill(&s, "UNIT-1", &[&hub, &source], RECONCILED_AT);

    let out = call(&s, "get", json!({"bank": "bank-a", "item_id": "UNIT-1"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let block = &out["freshness"];
    assert_eq!(block["proof_count"], json!(2), "{out}");
    assert_eq!(block["last_reconciled_at"], json!(RECONCILED_AT), "{out}");
    assert_eq!(block["stale"], json!(false), "{out}");
    assert_eq!(block["stale_reason"], json!(null), "{out}");

    let trail = block["evidence"].as_array().expect("evidence trail");
    assert_eq!(trail.len(), 1, "{out}");
    assert_eq!(trail[0]["revision"], json!(1), "{out}");
    assert_eq!(trail[0]["reconciled_at"], json!(RECONCILED_AT), "{out}");
    // Ordered by source id, so the trail cannot depend on scan order.
    let entries = trail[0]["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 2, "{out}");
    assert_eq!(entries[0]["source_id"], json!("HUB-1"), "{out}");
    assert_eq!(entries[0]["as_of"], json!(SOURCE_AS_OF), "{out}");
    assert_eq!(entries[1]["source_id"], json!("SRC-1"), "{out}");
}

#[test]
fn get_adds_only_the_freshness_key_to_a_unit_response() {
    let s = state();
    let (hub, source, _) = hub_bank(&s);
    distill(&s, "UNIT-1", &[&hub, &source], RECONCILED_AT);

    // Two items of identical shape, one distilled and one not: the only
    // difference a read can show between them is the added key.
    let unit = call(&s, "get", json!({"bank": "bank-a", "item_id": "UNIT-1"}));
    let plain = call(&s, "get", json!({"bank": "bank-a", "item_id": "PLAIN-1"}));
    let mut expected = keys_of(&plain);
    expected.push("freshness".to_owned());
    expected.sort();
    assert_eq!(keys_of(&unit), expected, "exactly one key is added: {unit}");
    assert!(
        plain.get("freshness").is_none(),
        "a non-unit read is unchanged: {plain}"
    );
    assert_eq!(unit["gist"], json!(UNIT_GIST), "{unit}");
    // The item's own halves are what they were: the block sits beside them and
    // rewrites neither the prose nor the snapshot.
    assert_eq!(unit["snapshot"]["unit_id"], json!("UNIT-1"), "{unit}");
    assert_eq!(unit["snapshot"]["hub_id"], json!("HUB-1"), "{unit}");
    assert_eq!(
        unit["source_ref"],
        json!(clio_retrieve::HUB_DISTILL_SOURCE_REF),
        "{unit}"
    );
}

#[test]
fn get_resolves_a_revised_units_record_through_its_snapshot() {
    let s = state();
    let (hub, source, _) = hub_bank(&s);
    let mut revised = distill(&s, "UNIT-1", &[&hub, &source], RECONCILED_AT);

    // Revision 2 moves the item id to `{unit}:v2` while the record keeps the
    // stable unit id, so only the snapshot's `unit_id` can find it.
    revised.revision = 2;
    revised.distilled_at = REVISED_AT.to_owned();
    revised.record_evidence(evidence_from_items([&hub, &source]), REVISED_AT);
    s.store
        .revise_distilled_unit(&revised, &unit_item(&revised), "tester")
        .expect("revise");

    let out = call(&s, "get", json!({"bank": "bank-a", "item_id": "UNIT-1:v2"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["id"], json!("UNIT-1:v2"), "{out}");
    assert_eq!(out["snapshot"]["unit_id"], json!("UNIT-1"), "{out}");
    assert_eq!(
        out["freshness"]["revision"],
        json!(2),
        "the block describes revision 2, not the record key: {out}"
    );
    assert_eq!(out["freshness"]["stale"], json!(false), "{out}");
    assert_eq!(
        out["freshness"]["last_reconciled_at"],
        json!(REVISED_AT),
        "{out}"
    );
    // Both reconciliations stay readable: a later revision never rewrites the
    // list the earlier one was built from.
    let trail = out["freshness"]["evidence"]
        .as_array()
        .expect("evidence trail");
    assert_eq!(trail.len(), 2, "{out}");
    assert_eq!(trail[0]["revision"], json!(1), "{out}");
    assert_eq!(trail[0]["reconciled_at"], json!(RECONCILED_AT), "{out}");
    assert_eq!(trail[1]["revision"], json!(2), "{out}");
    assert_eq!(trail[1]["reconciled_at"], json!(REVISED_AT), "{out}");
}

#[test]
fn an_item_whose_snapshot_names_no_record_gains_no_freshness_key() {
    let s = state();
    // A hub-shaped item with no derivation record. The snapshot alone must not
    // conjure a block, or a fabricated `unit_id` would report a unit that does
    // not exist.
    let mut orphan = semantic_item("ORPHAN-1", "bank-a", "alpha orphan", SOURCE_AS_OF);
    orphan.source_ref = Some(clio_retrieve::HUB_DISTILL_SOURCE_REF.into());
    orphan.snapshot = Some(json!({"unit_id": "NO-SUCH-UNIT", "hub_id": "HUB-1"}));
    put(&s, &orphan, "alpha orphan");

    let out = call(&s, "get", json!({"bank": "bank-a", "item_id": "ORPHAN-1"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(out.get("freshness").is_none(), "no record, no block: {out}");
}

#[test]
fn a_unit_whose_source_left_active_use_names_the_removal() {
    let s = state();
    let (hub, source, _) = hub_bank(&s);
    distill(&s, "UNIT-1", &[&hub, &source], RECONCILED_AT);

    // The real path: an operations removal cascades onto the claiming unit.
    let discarded = call(
        &s,
        "discard",
        json!({
            "bank": "bank-a", "item_id": "SRC-1", "reason": "ops", "confirm": true,
            "timestamp": "2026-01-04T00:00:00Z"
        }),
    );
    assert_eq!(discarded["ok"], json!(true), "{discarded}");

    let out = call(&s, "get", json!({"bank": "bank-a", "item_id": "UNIT-1"}));
    assert_eq!(out["freshness"]["stale"], json!(true), "{out}");
    assert_eq!(
        out["freshness"]["stale_reason"],
        json!("source_removed"),
        "{out}"
    );
}

#[test]
fn a_unit_with_an_unstamped_source_reads_as_unknown_not_current() {
    let s = state();
    let hub = semantic_item("HUB-1", "bank-a", "alpha hub note", SOURCE_AS_OF);
    // The write path did not stamp this source, so the comparison cannot be
    // made. Unknown must never render as current.
    let unstamped = semantic_item("SRC-1", "bank-a", "alpha source note", UNSTAMPED);
    put(&s, &hub, "alpha hub note");
    put(&s, &unstamped, "alpha source note");
    distill(&s, "UNIT-1", &[&hub, &unstamped], RECONCILED_AT);

    let out = call(&s, "get", json!({"bank": "bank-a", "item_id": "UNIT-1"}));
    assert_eq!(out["freshness"]["stale"], json!(null), "{out}");
    assert_eq!(out["freshness"]["stale_reason"], json!(null), "{out}");
    // The evidence entry keeps the absent stamp visible instead of inventing one.
    let entries = out["freshness"]["evidence"][0]["entries"]
        .as_array()
        .expect("entries");
    assert_eq!(entries[1]["source_id"], json!("SRC-1"), "{out}");
    assert_eq!(entries[1]["as_of"], json!(null), "{out}");
}

#[test]
fn the_freshness_block_carries_ids_and_times_only() {
    let s = state();
    let hub = semantic_item("HUB-1", "bank-a", "alpha hub note", SOURCE_AS_OF);
    let source = semantic_item("SRC-1", "bank-a", "secret source note", SOURCE_AS_OF);
    put(&s, &hub, "alpha hub note");
    put(&s, &source, "secret source note");
    distill(&s, "UNIT-1", &[&hub, &source], RECONCILED_AT);

    let out = call(&s, "get", json!({"bank": "bank-a", "item_id": "UNIT-1"}));
    let block = &out["freshness"];
    assert_eq!(
        keys_of(block),
        vec![
            "evidence",
            "last_reconciled_at",
            "proof_count",
            "revision",
            "stale",
            "stale_reason"
        ],
        "the block's key set is fixed: {block}"
    );
    assert_eq!(
        block["revision"],
        json!(1),
        "the block names the revision it describes: {block}"
    );
    let trail = block["evidence"].as_array().expect("evidence trail");
    assert_eq!(
        keys_of(&trail[0]),
        vec!["entries", "reconciled_at", "revision"],
        "{trail:?}"
    );
    for entry in trail[0]["entries"].as_array().expect("entries") {
        assert_eq!(
            keys_of(entry),
            vec!["as_of", "source_id"],
            "an evidence entry is an id and a time: {entry}"
        );
    }
    // No source text and no gist reaches the block: a caller authorized for the
    // unit learns nothing about its sources beyond the ids and times the unit's
    // own snapshot already names.
    let text = serde_json::to_string(block).expect("block text");
    assert!(!text.contains("secret source note"), "{text}");
    assert!(!text.contains(UNIT_GIST), "{text}");
}

#[test]
fn inspect_reports_freshness_on_a_consolidated_row_only() {
    let s = state();
    let (hub, source, _) = hub_bank(&s);
    distill(&s, "UNIT-1", &[&hub, &source], RECONCILED_AT);

    let out = call(&s, "inspect", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let rows = out["items"].as_array().expect("rows");
    let row = |id: &str| {
        rows.iter()
            .find(|r| r["item_id"] == json!(id))
            .unwrap_or_else(|| panic!("row {id} missing: {out}"))
            .clone()
    };
    let unit = row("UNIT-1");
    assert_eq!(unit["freshness"]["proof_count"], json!(2), "{out}");
    assert_eq!(unit["freshness"]["stale"], json!(false), "{out}");
    // The row's own metadata keys are untouched: only the block was added.
    let mut expected = keys_of(&row("PLAIN-1"));
    expected.push("freshness".to_owned());
    expected.sort();
    assert_eq!(keys_of(&unit), expected, "{out}");
}

#[test]
fn a_recall_hit_carries_no_freshness_field() {
    let s = state();
    let (hub, source, _) = hub_bank(&s);
    distill(&s, "UNIT-1", &[&hub, &source], RECONCILED_AT);
    s.index
        .put_lexical_doc("bank-a", "UNIT-1", "alpha summarized cluster")
        .expect("lexical doc");

    let out = call(&s, "retrieve", json!({"bank": "bank-a", "query": "alpha"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let hits = out["hits"].as_array().expect("hits");
    let hit = hits
        .iter()
        .find(|h| h["item_id"] == json!("UNIT-1"))
        .unwrap_or_else(|| panic!("unit not recalled: {out}"));
    // Freshness is read through `get`/`inspect`; the recall hit shape is frozen
    // and gains no field for it, current or lapsed.
    assert!(
        hit.get("freshness").is_none(),
        "recall hit shape is unchanged: {hit}"
    );
    for key in keys_of(hit) {
        assert!(
            !key.contains("stale") && !key.contains("proof") && !key.contains("reconcil"),
            "unexpected recall field `{key}`: {hit}"
        );
    }
}
