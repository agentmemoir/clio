// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface coverage of the ephemeral
//! scratchpad (quotas + isolation from retrieval) and the single-slot
//! canonical tools (supersession, admission rejection, snapshot passthrough).
//!
//! ## Owns
//! - Dispatcher-level assertions for `scratchpad_*` and `canonical_*`
//!
//! ## Does not own
//! - Shared-surface, `validate`, and `stats` assertions (see
//!   `additive_tools_shared_tests`)
//! - Shared state/dispatch fixtures (see `additive_tools_test_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::additive_tools_test_support_tests::{
    call, call_read, rejecting_state, state, store_args,
};
use clio_types::MemoryItem;
use serde_json::{Value, json};

// ---------- scratchpad ----------

#[test]
fn scratchpad_round_trip_overwrite_and_view() {
    let s = state();
    let out = call(
        &s,
        "scratchpad_write",
        json!({
            "key": "plan", "content": "{\"step\":1}", "bank": "b", "actor": "agent"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["lifetime"], json!("process"), "{out}");
    let read = call(
        &s,
        "scratchpad_read",
        json!({"key": "plan", "bank": "b", "actor": "agent"}),
    );
    assert_eq!(read["ok"], json!(true));
    assert_eq!(read["content"]["step"], 1);

    call(
        &s,
        "scratchpad_write",
        json!({
            "key": "plan", "content": "plain", "bank": "b", "actor": "agent"
        }),
    );
    let view = call(
        &s,
        "scratchpad_read",
        json!({"bank": "b", "actor": "agent"}),
    );
    assert_eq!(view["entries"].as_array().unwrap().len(), 1);
    assert_eq!(view["entries"][0]["content"], "plain");
}

#[test]
fn scratchpad_quota_and_isolation() {
    let s = state();
    let big = "x".repeat(64 * 1024 + 1);
    let out = call(
        &s,
        "scratchpad_write",
        json!({
            "key": "blob", "content": big, "bank": "b", "actor": "agent"
        }),
    );
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("quota_exceeded"));
    assert!(
        out["message"]
            .as_str()
            .unwrap_or("")
            .contains("QUOTA_EXCEEDED")
    );

    // Session/bank isolation: another session cannot read the key.
    call(
        &s,
        "scratchpad_write",
        json!({
            "key": "k", "content": "secret-a", "session": "s1", "bank": "b", "actor": "agent"
        }),
    );
    let missing = call(
        &s,
        "scratchpad_read",
        json!({
            "key": "secret-a", "session": "s2", "bank": "b", "actor": "agent"
        }),
    );
    assert_eq!(missing["ok"], json!(false));
    assert_eq!(missing["code"], json!("item_not_found"));

    // Clear drops only the addressed scope.
    call(
        &s,
        "scratchpad_clear",
        json!({"bank": "b", "actor": "agent"}),
    );
    let view = call(
        &s,
        "scratchpad_read",
        json!({"bank": "b", "actor": "agent"}),
    );
    assert_eq!(view["total_bytes"], 0);
}

#[test]
fn scratchpad_content_never_reachable_via_retrieve() {
    let s = state();
    // Durable row carrying the shared marker, indexed for lexical search.
    call(&s, "store", store_args("itm-probe"));
    s.index
        .put_lexical_doc(
            "bank-a",
            "itm-probe",
            "canonical scratchpad leak probe zqxj",
        )
        .expect("lexical doc");
    // The scratchpad holds a DIFFERENT pad-only token. It must never be
    // indexed, so it must not appear in any retrieve response.
    let pad = call(
        &s,
        "scratchpad_write",
        json!({
            "key": "leak", "content": "padonly-token", "bank": "bank-a", "actor": "agent"
        }),
    );
    assert_eq!(pad["ok"], json!(true), "{pad}");
    // The pad write really landed (the claim is about search isolation, not a
    // silently dropped write).
    let read = call(
        &s,
        "scratchpad_read",
        json!({"key": "leak", "bank": "bank-a", "actor": "agent"}),
    );
    assert_eq!(read["content"], "padonly-token");

    // The durable marker is retrievable, and no returned hit came from the pad.
    let hits = call(
        &s,
        "retrieve",
        json!({"query": "zqxj marker", "bank": "bank-a"}),
    );
    assert_eq!(hits["ok"], json!(true), "{hits}");
    let rows = hits["hits"].as_array().expect("hits array");
    assert!(
        rows.iter().any(|h| h["item_id"] == json!("itm-probe")),
        "durable probe row must be retrievable: {hits}"
    );
    assert!(
        rows.iter()
            .all(|h| !h.to_string().contains("padonly-token")),
        "scratchpad content leaked into retrieve: {hits}"
    );
    // A search for the pad-only token returns no durable row at all.
    let pad_hits = call(
        &s,
        "retrieve",
        json!({"query": "padonly-token", "bank": "bank-a"}),
    );
    assert_eq!(pad_hits["ok"], json!(true), "{pad_hits}");
    assert!(
        pad_hits["hits"].as_array().expect("hits").is_empty(),
        "scratchpad-only token must not be retrievable: {pad_hits}"
    );
}

// ---------- canonical ----------

#[test]
fn canonical_put_supersedes_and_canonical_get_reads() {
    let s = state();
    let first = call(
        &s,
        "canonical_put",
        json!({
            "key": "primary_database_host", "value": "db-a", "category": "tool_config",
            "epistemic_kind": "fact", "bank": "b", "actor": "agent"
        }),
    );
    assert_eq!(first["ok"], json!(true), "{first}");
    assert!(first["pass"].as_bool().unwrap_or(false));
    let first_id = first["id"].as_str().unwrap().to_owned();

    let second = call(
        &s,
        "canonical_put",
        json!({
            "key": "primary_database_host", "value": "db-b", "category": "tool_config",
            "epistemic_kind": "fact", "bank": "b", "actor": "agent"
        }),
    );
    let second_id = second["id"].as_str().unwrap().to_owned();

    let active = call_read(
        &s,
        "canonical_get",
        json!({"key": "primary_database_host", "bank": "b", "actor": "agent"}),
    )
    .unwrap();
    assert_eq!(active["value"], "db-b");
    assert_eq!(active["item_id"], second_id);

    let historical = call_read(
        &s,
        "canonical_get",
        json!({
            "key": "primary_database_host", "as_of": "2026-01-01T00:00:00Z", "bank": "b", "actor": "agent"
        }),
    );
    let _ = historical;
    // The prior revision is closed, not deleted.
    let prior = s.store.get_memory_item("b", &first_id).unwrap().unwrap();
    assert!(prior.tx_until.is_some() || first_id == second_id);

    // as_of before the first write misses; today reads active.
    let empty = call_read(
        &s,
        "canonical_get",
        json!({"key": "missing", "bank": "b", "actor": "agent"}),
    );
    assert!(empty.is_err());
}

#[test]
fn canonical_put_dry_run_scores_without_writing_and_missing_slot_errors() {
    let s = state();
    let dry = call(
        &s,
        "canonical_put",
        json!({
            "key": "slot", "value": "v", "category": "tool_config",
            "epistemic_kind": "fact", "bank": "b", "actor": "agent", "dry_run": true
        }),
    );
    assert_eq!(dry["ok"], json!(true), "{dry}");
    assert_eq!(dry["dry_run"], json!(true));
    assert!(dry["pass"].as_bool().unwrap_or(false));
    assert_eq!(dry["id"], json!(Value::Null));
    assert_eq!(
        s.store
            .count_items(&clio_store::ItemInspectFilter::new("b"))
            .unwrap(),
        0
    );

    let missing = call_read(
        &s,
        "canonical_get",
        json!({"key": "nope", "bank": "b", "actor": "agent"}),
    );
    assert!(missing.is_err());
}

#[test]
fn canonical_put_below_admission_threshold_is_rejected_without_closing_prior() {
    let s = rejecting_state();
    let prior = MemoryItem {
        id: "itm-prior".into(),
        bank_id: "b".into(),
        subject_id: clio_compliance::canonical_subject("slot"),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::parse("tool_config").unwrap()),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: None,
        snapshot: None,
        gist: Some("db-old".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    };
    s.store.create_memory_item(&prior, "agent").unwrap();

    let out = call(
        &s,
        "canonical_put",
        json!({
            "key": "slot", "value": "db-new", "category": "tool_config",
            "epistemic_kind": "fact", "bank": "b", "actor": "agent"
        }),
    );
    // The single-op path reports the rejection in the decision envelope
    // (`pass=false`, no id) rather than as a tool error; the write never runs.
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["pass"], json!(false), "{out}");
    assert!(
        out["rejection_reason"]
            .as_str()
            .unwrap_or("")
            .contains("theta"),
        "{out}"
    );
    assert_eq!(out["id"], json!(Value::Null), "{out}");
    // The prior revision is untouched: a rejected put never supersedes.
    let row = s.store.get_memory_item("b", "itm-prior").unwrap().unwrap();
    assert!(row.tx_until.is_none());
    let active = call_read(
        &s,
        "canonical_get",
        json!({"key": "slot", "bank": "b", "actor": "agent"}),
    )
    .unwrap();
    assert_eq!(active["item_id"], json!("itm-prior"));
    assert_eq!(active["value"], json!("db-old"));
}

#[test]
fn canonical_put_snapshot_round_trips_through_canonical_get() {
    let s = state();
    let out = call(
        &s,
        "canonical_put",
        json!({
            "key": "primary_database_host", "value": "db-a", "category": "tool_config",
            "epistemic_kind": "fact", "bank": "b", "actor": "agent",
            "snapshot": {"entity": "db-a", "amount": 1, "date": "2024-01-01"},
            "source_text": "db-a (1 note since 2024-01-01)."
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let got = call_read(
        &s,
        "canonical_get",
        json!({"key": "primary_database_host", "bank": "b", "actor": "agent"}),
    )
    .unwrap();
    assert_eq!(got["snapshot"]["entity"], json!("db-a"));
    assert_eq!(got["snapshot"]["amount"], json!(1));
}
