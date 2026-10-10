// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the lossless read regression: `get_snapshot`
//! and `inspect` must still return the whole stored record unchanged now that
//! a retrieval hit may also carry a bounded rendering.
//!
//! ## Owns
//! - The `get_snapshot` regression: the stored snapshot comes back whole,
//!   byte-for-byte, whatever the retrieval hit's rendering does
//! - The `inspect` regression: the listing still carries every metadata column
//!   and the item's own `context`, with no rendering added to either
//!
//! ## Does not own
//! - The explain-trace boundary sweep (`read_explain_guard_tests`)
//! - Retrieval rendering, the byte cap, or the rendering's attachment
//!   (`clio-retrieve`)
//! - The item read handlers themselves (`read_tools`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the lossless read paths.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

const BANK: &str = "full-record-mcp";

/// A synthetic marker standing in for the content the lossy half summarizes.
/// No real item text is used anywhere in this file.
const RECORD: &str = "FullRecordSynthetic74";

/// Hermetic state: no deployment overlay, so no embedder is resolved and the
/// test depends on no sidecar.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: BANK.into(),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("state")
}

/// One item whose snapshot is deep enough to be worth reading whole.
fn item(id: &str, snapshot: Value) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: "subj-full".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: Some("turn-full".into()),
        snapshot: Some(snapshot),
        gist: Some(format!("full record note {RECORD}")),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: Some(format!("full record context {RECORD}")),
    }
}

/// The synthetic snapshot: nested objects, an array, and the marker, so a
/// truncated or flattened read would differ from the whole one.
fn snapshot() -> Value {
    json!({
        "entity": RECORD,
        "amount": 42,
        "date": "2024-01-01",
        "notes": ["first synthetic note", "second synthetic note"],
        "nested": {"unit": "u-synthetic", "proofs": ["p-a", "p-b"]}
    })
}

/// A retrieval hit's rendering is bounded and lossy, so the lossless read has
/// to stay whole on its own. This is the regression: the rendering's arrival
/// must not have narrowed what `get_snapshot` returns.
#[test]
fn get_snapshot_returns_the_whole_stored_record_unchanged() {
    let state = state();
    let stored = snapshot();
    state
        .store
        .create_memory_item(&item("fr-1", stored.clone()), "agent")
        .expect("create item");

    let out = dispatch(
        &state,
        "get_snapshot",
        &json!({"bank": BANK, "item_id": "fr-1"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["item_id"], json!("fr-1"), "{out}");
    assert_eq!(
        out["authoritative"],
        json!(true),
        "the lossless read stays authoritative: {out}"
    );
    assert_eq!(
        out["snapshot"], stored,
        "the stored record must come back whole, key for key and value for value"
    );

    // Whole means every key survived, including the nested ones a bounded
    // rendering would drop or flatten.
    let keys: BTreeSet<&str> = stored
        .as_object()
        .expect("snapshot object")
        .keys()
        .map(String::as_str)
        .collect();
    let returned = out["snapshot"].as_object().expect("snapshot object");
    for key in &keys {
        assert!(
            returned.contains_key(*key),
            "snapshot key `{key}` is missing from the lossless read: {out}"
        );
    }
    assert_eq!(
        returned["nested"]["proofs"],
        json!(["p-a", "p-b"]),
        "a nested array must survive the lossless read: {out}"
    );
}

/// `inspect` lists the item's own metadata and context and must not gain a
/// rendering column: the listing is a different surface from the retrieval
/// hit, and the rendering belongs to neither.
#[test]
fn inspect_still_returns_the_full_metadata_row_unchanged() {
    let state = state();
    state
        .store
        .create_memory_item(&item("fr-2", snapshot()), "agent")
        .expect("create item");

    let out = dispatch(&state, "inspect", &json!({"bank": BANK}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["total"], json!(1), "{out}");
    let rows = out["items"].as_array().expect("items");
    assert_eq!(rows.len(), 1, "{out}");
    let row = &rows[0];

    assert_eq!(row["item_id"], json!("fr-2"), "{out}");
    assert_eq!(row["kind"], json!("semantic"), "{out}");
    assert_eq!(row["category"], json!("schema"), "{out}");
    assert_eq!(row["epistemic_kind"], json!("fact"), "{out}");
    assert_eq!(row["source_ref"], json!("turn-full"), "{out}");
    assert_eq!(row["valid_from"], json!("2024-01-01T00:00:00Z"), "{out}");
    assert_eq!(row["tx_from"], json!("2024-01-01T00:00:00Z"), "{out}");
    assert_eq!(row["discarded"], json!(false), "{out}");
    assert_eq!(row["archived"], json!(false), "{out}");
    assert_eq!(
        row["context"],
        json!(format!("full record context {RECORD}")),
        "the item's own context must still be listed: {out}"
    );

    // The listing is a metadata surface, so it must not have gained the hit's
    // rendering, and the row must not have lost a column either.
    assert!(
        row.get("exact_render").is_none(),
        "the listing must not carry a rendering: {out}"
    );
    assert!(
        row.get("gist").is_none(),
        "the listing stays metadata-only and must not have gained a gist: {out}"
    );
    let columns: BTreeSet<&str> = row
        .as_object()
        .expect("row object")
        .keys()
        .map(String::as_str)
        .collect();
    for column in [
        "item_id",
        "kind",
        "category",
        "epistemic_kind",
        "admission_score",
        "source_ref",
        "valid_from",
        "tx_from",
        "created_at",
        "updated_at",
        "discarded",
        "archived",
    ] {
        assert!(
            columns.contains(column),
            "listing column `{column}` is missing: {out}"
        );
    }
}

/// The rendering and the stored record are two different things, and the
/// stored record is the authoritative one: the rendering is the snapshot's own
/// bytes collapsed into one string, while the lossless read returns it
/// structured. The regression this pins is that adding the rendering did not
/// make the structured form unreachable, so the lossy half is never the only
/// way in.
#[test]
fn the_rendering_is_one_string_and_the_stored_record_stays_structured() {
    let state = state();
    let stored = snapshot();
    state
        .store
        .create_memory_item(&item("fr-3", stored.clone()), "agent")
        .expect("create item");

    // The rendering, as the retrieve crate builds it from this stored value.
    let rendered = clio_retrieve::render_exact(&stored).expect("the synthetic snapshot renders");
    assert!(
        rendered.len() <= clio_retrieve::EXACT_RENDER_MAX_BYTES,
        "the rendering must fit the published cap, or the fixture proves nothing: {rendered}"
    );
    assert!(
        rendered.contains(RECORD),
        "the rendering summarizes the record's own content: {rendered}"
    );

    // The lossless read returns the same content structured, not as the
    // rendering's single string, and key for key with the stored value.
    let snap = dispatch(
        &state,
        "get_snapshot",
        &json!({"bank": BANK, "item_id": "fr-3"}),
    );
    assert_eq!(snap["ok"], json!(true), "{snap}");
    assert_eq!(
        snap["snapshot"], stored,
        "the lossless read is unchanged by the rendering's existence: {snap}"
    );
    assert!(
        snap["snapshot"].is_object(),
        "the lossless read stays structured, not a rendering string: {snap}"
    );
    assert_eq!(
        snap["snapshot"]["nested"]["proofs"],
        json!(["p-a", "p-b"]),
        "the nested shape the rendering flattens into text is still readable: {snap}"
    );
}
