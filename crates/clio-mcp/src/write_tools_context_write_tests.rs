// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface tests of the optional bounded
//! `context` field on the `store`, `admit_preview`, and `admit_preview_batch`
//! write paths.
//!
//! ## Owns
//! - `context` decoding and validation assertions on those tools
//! - Persistence, retrieval, and no-write assertions for a supplied context
//!
//! ## Does not own
//! - `canonical_put`, `shared_store`, and `batch` coverage (see
//!   `context_batch_canonical_tests`)
//! - `correct` coverage (see `context_correct_tests`)
//! - `inspect` and schema parity (see `context_read_schema_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_types::EpistemicKind;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn store_args(id: &str) -> Value {
    json!({
        "item": {
            "id": id,
            "subject_id": format!("subj-{id}"),
            "gist": format!("prefers terse answers {id}"),
            "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": format!("The user prefers terse answers {id} (1 note since 2024-01-01)."),
        "bank": "bank-a",
        "actor": "agent"
    })
}

fn bank_count(state: &McpState) -> usize {
    state
        .store
        .count_items(&clio_store::ItemInspectFilter::new("bank-a"))
        .expect("count")
}

#[test]
fn store_with_context_persists_and_get_returns_it() {
    let s = state();
    let mut args = store_args("itm-ctx");
    args["context"] = json!("team chat, 2026-09");
    let out = call(&s, "store", args);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["id"], json!("itm-ctx"));
    let stored = s
        .store
        .get_memory_item("bank-a", "itm-ctx")
        .expect("read back")
        .expect("stored");
    assert_eq!(stored.context.as_deref(), Some("team chat, 2026-09"));

    let get = call(&s, "get", json!({"bank": "bank-a", "item_id": "itm-ctx"}));
    assert_eq!(get["ok"], json!(true), "{get}");
    assert_eq!(get["context"], json!("team chat, 2026-09"));
}

#[test]
fn store_without_context_stores_none() {
    let s = state();
    let out = call(&s, "store", store_args("itm-nocontext"));
    assert_eq!(out["ok"], json!(true), "{out}");
    let stored = s
        .store
        .get_memory_item("bank-a", "itm-nocontext")
        .expect("read back")
        .expect("stored");
    assert!(stored.context.is_none());

    let get = call(
        &s,
        "get",
        json!({"bank": "bank-a", "item_id": "itm-nocontext"}),
    );
    assert_eq!(get["ok"], json!(true), "{get}");
    assert!(
        get.get("context").is_none(),
        "absent context must not serialize: {get}"
    );
}

#[test]
fn empty_context_rejected_without_write_and_null_is_absent() {
    let s = state();
    let mut empty = store_args("itm-empty");
    empty["context"] = json!("");
    let out = call(&s, "store", empty);
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"));
    assert!(
        s.store
            .get_memory_item("bank-a", "itm-empty")
            .expect("read back")
            .is_none()
    );
    assert_eq!(bank_count(&s), 0);

    // JSON null means "absent", exactly like an omitted key.
    let mut null = store_args("itm-null");
    null["context"] = Value::Null;
    let ok = call(&s, "store", null);
    assert_eq!(ok["ok"], json!(true), "{ok}");
    let stored = s
        .store
        .get_memory_item("bank-a", "itm-null")
        .expect("read back")
        .expect("stored");
    assert!(stored.context.is_none());
}

#[test]
fn context_byte_limit_enforced_and_exact_limit_accepted() {
    let s = state();
    let mut over = store_args("itm-over");
    over["context"] = json!("a".repeat(4097));
    let out = call(&s, "store", over);
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("out_of_range"));
    assert!(
        s.store
            .get_memory_item("bank-a", "itm-over")
            .expect("read back")
            .is_none()
    );

    let mut exact = store_args("itm-exact");
    exact["context"] = json!("a".repeat(4096));
    let ok = call(&s, "store", exact);
    assert_eq!(ok["ok"], json!(true), "{ok}");
    let stored = s
        .store
        .get_memory_item("bank-a", "itm-exact")
        .expect("read back")
        .expect("stored");
    assert_eq!(stored.context.as_deref().map(str::len), Some(4096));

    // The limit is bytes, not characters: 2048 two-byte chars fit, 2049 do not.
    let mut multibyte = store_args("itm-multibyte");
    multibyte["context"] = json!("é".repeat(2048));
    let accepted = call(&s, "store", multibyte);
    assert_eq!(accepted["ok"], json!(true), "{accepted}");
    let mut multibyte_over = store_args("itm-multibyte-over");
    multibyte_over["context"] = json!("é".repeat(2049));
    let rejected = call(&s, "store", multibyte_over);
    assert_eq!(rejected["ok"], json!(false), "{rejected}");
    assert_eq!(rejected["code"], json!("out_of_range"));
}

#[test]
fn non_string_context_rejected_as_invalid_type() {
    let s = state();
    for bad in [json!(42), json!({"note": "x"}), json!(["a"])] {
        let mut args = store_args("itm-badtype");
        args["context"] = bad;
        let out = call(&s, "store", args);
        assert_eq!(out["ok"], json!(false), "{out}");
        assert_eq!(out["code"], json!("invalid_type"), "{out}");
    }
    assert_eq!(bank_count(&s), 0);
}

#[test]
fn store_context_does_not_disturb_existing_evidence_ref_behavior() {
    let s = state();
    let mut args = store_args("itm-evidence");
    args["evidence_ref"] = json!("ev-42");
    args["context"] = json!("handover note");
    let out = call(&s, "store", args);
    assert_eq!(out["ok"], json!(true), "{out}");
    let stored = s
        .store
        .get_memory_item("bank-a", "itm-evidence")
        .expect("read back")
        .expect("stored");
    assert_eq!(stored.source_ref.as_deref(), Some("ev-42"));
    assert_eq!(stored.context.as_deref(), Some("handover note"));
}

#[test]
fn fact_with_context_stays_fact_and_belief_rules_hold() {
    let s = state();
    let mut fact = store_args("itm-fact");
    fact["context"] = json!("ops handover note");
    let out = call(&s, "store", fact);
    assert_eq!(out["ok"], json!(true), "{out}");
    let stored = s
        .store
        .get_memory_item("bank-a", "itm-fact")
        .expect("read back")
        .expect("stored");
    assert_eq!(stored.context.as_deref(), Some("ops handover note"));
    assert_eq!(stored.epistemic_kind, EpistemicKind::Fact);
    assert_eq!(stored.source_type, None, "context must not add source_type");

    // A fact still rejects an explicit source_type with context present.
    let mut bad_fact = store_args("itm-badfact");
    bad_fact["source_type"] = json!("agent_inferred");
    bad_fact["context"] = json!("still only context");
    let rejected = call(&s, "store", bad_fact);
    assert_eq!(rejected["ok"], json!(false), "{rejected}");

    // A belief still requires source_type, with or without context.
    let mut belief = store_args("itm-belief");
    belief["epistemic_kind"] = json!("belief");
    belief["context"] = json!("context on a belief");
    let missing = call(&s, "store", belief.clone());
    assert_eq!(missing["ok"], json!(false), "{missing}");

    belief["source_type"] = json!("user_stated");
    let admitted = call(&s, "store", belief);
    assert_eq!(admitted["ok"], json!(true), "{admitted}");
    let stored_belief = s
        .store
        .get_memory_item("bank-a", "itm-belief")
        .expect("read back")
        .expect("stored");
    assert_eq!(
        stored_belief.context.as_deref(),
        Some("context on a belief")
    );
}

#[test]
fn context_never_satisfies_snapshot_span_verification() {
    let s = state();
    // Decoy: the snapshot amount appears only in `context`, never in source_text.
    let mut decoy = store_args("itm-span-decoy");
    decoy["item"]["snapshot"] =
        json!({"entity": "terse answers", "amount": 7, "date": "2024-01-01"});
    decoy["context"] = json!("The recorded amount is exactly 7.");
    let out = call(&s, "store", decoy);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(
        !out["pass"].as_bool().unwrap_or(true),
        "decoy context must not verify the snapshot span: {out}"
    );
    let reason = out["rejection_reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("span_verify") && reason.contains("amount"),
        "{out}"
    );
    assert!(
        s.store
            .get_memory_item("bank-a", "itm-span-decoy")
            .expect("read back")
            .is_none()
    );

    // Positive control: the same decoy context stays irrelevant while the
    // matching snapshot verifies against source_text alone.
    let mut good = store_args("itm-span-good");
    good["context"] = json!("The recorded amount is exactly 7.");
    let admitted = call(&s, "store", good);
    assert_eq!(admitted["ok"], json!(true), "{admitted}");
    assert!(admitted["pass"].as_bool().unwrap_or(false), "{admitted}");
}

#[test]
fn admit_preview_with_context_does_not_write() {
    let s = state();
    let mut args = store_args("itm-preview-ctx");
    args["context"] = json!("preview-only context");
    let out = call(&s, "admit_preview", args);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(out["pass"].as_bool().unwrap_or(false), "{out}");
    assert_eq!(out["dry_run"], json!(true));
    assert!(
        s.store
            .get_memory_item("bank-a", "itm-preview-ctx")
            .expect("read back")
            .is_none()
    );
    assert_eq!(bank_count(&s), 0);

    // A malformed context fails identically on the no-write preview.
    let mut bad = store_args("itm-preview-bad");
    bad["context"] = json!("");
    let rejected = call(&s, "admit_preview", bad);
    assert_eq!(rejected["ok"], json!(false), "{rejected}");
    assert_eq!(rejected["code"], json!("invalid_argument"));
}

#[test]
fn admit_preview_batch_items_accept_context_without_writing() {
    let s = state();
    let out = call(
        &s,
        "admit_preview_batch",
        json!({
            "bank": "bank-a",
            "actor": "agent",
            "items": [
                {
                    "item": {
                        "id": "itm-b1",
                        "subject_id": "s1",
                        "gist": "g1",
                        "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
                    },
                    "category": "persona",
                    "epistemic_kind": "fact",
                    "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
                    "context": "batch item context one"
                },
                {
                    "item": {"id": "itm-b2", "subject_id": "s2", "gist": "g2"},
                    "category": "persona",
                    "epistemic_kind": "fact",
                    "context": ""
                }
            ]
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["writes"], json!(0));
    assert_eq!(out["items"][0]["pass"], json!(true), "{out}");
    assert_eq!(out["items"][1]["pass"], json!(false), "{out}");
    assert_eq!(
        out["items"][1]["error"]["code"],
        json!("invalid_argument"),
        "{out}"
    );
    assert_eq!(bank_count(&s), 0);
}
