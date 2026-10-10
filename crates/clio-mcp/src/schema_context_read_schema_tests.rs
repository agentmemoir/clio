// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface tests of `inspect` context
//! exposure (including unreadable sealed bodies: crypto-shredded subjects,
//! legacy opaque rows, and undecryptable ciphertext) and for published-schema
//! parity of the bounded `context` property.
//!
//! ## Owns
//! - `inspect` row enrichment and unreadable-body survival assertions
//! - `tool_schema` context property assertions for the store/admit/correct set
//!
//! ## Does not own
//! - Runtime decoding of `context` (see `context_write_tests`)
//! - Store-level context sealing (see `clio-store` tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn store_item(state: &McpState, id: &str, subject: &str, context: Option<&str>) {
    let mut args = json!({
        "item": {
            "id": id,
            "subject_id": subject,
            "gist": format!("prefers terse answers {id}"),
            "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": format!("The user prefers terse answers {id} (1 note since 2024-01-01)."),
        "bank": "bank-a",
        "actor": "agent"
    });
    if let Some(context) = context {
        args["context"] = json!(context);
    }
    let out = call(state, "store", args);
    assert_eq!(out["ok"], json!(true), "store {id}: {out}");
}

fn row<'a>(items: &'a [Value], id: &str) -> &'a Value {
    items
        .iter()
        .find(|r| r["item_id"] == json!(id))
        .unwrap_or_else(|| panic!("row {id} missing"))
}

#[test]
fn inspect_returns_context_and_omits_absent_context() {
    let s = state();
    store_item(&s, "itm-with", "subj-with", Some("inspect context"));
    store_item(&s, "itm-without", "subj-without", None);

    let out = call(&s, "inspect", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let items = out["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(row(items, "itm-with")["context"], json!("inspect context"));
    assert!(
        row(items, "itm-without").get("context").is_none(),
        "items without context must not gain the field: {out}"
    );
}

#[test]
fn inspect_survives_a_crypto_shredded_subject() {
    let s = state();
    store_item(&s, "itm-shredded", "subj-shredded", Some("must vanish"));
    store_item(&s, "itm-alive", "subj-alive", Some("still readable"));

    let erased = call(
        &s,
        "erase_request",
        json!({
            "subject_id": "subj-shredded",
            "legal_basis": "test erasure",
            "request_id": "req-shred-1",
            "confirm": true,
            "bank": "bank-a",
            "actor": "harness"
        }),
    );
    assert_eq!(erased["ok"], json!(true), "{erased}");

    // The metadata row survives; its content (and context) is unreadable, so
    // the listing omits context for that row instead of failing.
    let out = call(&s, "inspect", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let items = out["items"].as_array().expect("items");
    assert_eq!(
        items.len(),
        2,
        "shredded metadata row must still list: {out}"
    );
    let shredded = row(items, "itm-shredded");
    assert!(
        shredded.get("context").is_none(),
        "shredded subject must omit context: {out}"
    );
    assert_eq!(row(items, "itm-alive")["context"], json!("still readable"));
}

#[test]
fn inspect_survives_an_unreadable_legacy_row() {
    let s = state();
    store_item(
        &s,
        "itm-readable",
        "subj-readable",
        Some("readable context"),
    );
    // A legacy-format row is not DualContent v1, so its sealed body cannot be
    // parsed; the listing must keep its metadata row and omit context rather
    // than failing the whole call. Such a row arrives only through the opaque
    // restore route, so the fixture restores one whose body decrypts under the
    // local DEK but is not DualContent v1.
    let cipher = s
        .kms
        .encrypt("subj-readable", b"legacy opaque bytes")
        .expect("legacy envelope")
        .to_storage_string();
    clio_store::BatchStore::apply_batch(
        &*s.batch,
        &[clio_store::BatchWrite::PutOpaqueItem {
            item: clio_store::OpaqueItemRecord {
                id: "itm-legacy-opaque".into(),
                bank_id: "bank-a".into(),
                subject_id: "subj-readable".into(),
                kind: "episodic".into(),
                category: None,
                episodic_type: Some("temporal".into()),
                epistemic_kind: "fact".into(),
                source_type: None,
                confidence: None,
                admission_score: None,
                source_ref: None,
                valid_from: "2024-01-01T00:00:00Z".into(),
                valid_until: None,
                tx_from: "2024-01-01T00:00:00Z".into(),
                tx_until: None,
                created_at: "2024-01-01T00:00:00Z".into(),
                updated_at: "2024-01-01T00:00:00Z".into(),
                content_cipher: cipher,
            },
            actor: "agent".into(),
        }],
    )
    .expect("legacy-format row restore");

    let out = call(&s, "inspect", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let items = out["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "both rows must still list: {out}");
    assert!(
        row(items, "itm-legacy-opaque").get("context").is_none(),
        "an unreadable legacy row must omit context: {out}"
    );
    assert_eq!(
        row(items, "itm-readable")["context"],
        json!("readable context"),
        "the readable row keeps its context: {out}"
    );
}

#[test]
fn inspect_survives_undecryptable_ciphertext_rows() {
    // Hermetic sealing: the process test key, never the machine's key store.
    clio_config::secret_seal::test_key::install_process_key(
        [0x6b; clio_config::secret_key::KEY_LEN],
    );
    // Items sealed under one key store must not abort the listing when a later
    // state over the same database cannot open their bodies: `subj-no-dek` and
    // `subj-alive` have no DEK in the fresh key store (`forbidden` "no DEK for
    // subject"), and `subj-tampered` gets a freshly minted DEK, so its old
    // envelope fails AEAD authentication (`forbidden` "content decryption
    // failed"). All three are Forbidden-classified by the KMS, yet none is an
    // authorization result — the listing keeps every metadata row and omits
    // their `context` (readable rows keep it; see the other tests here).
    let dir = std::env::temp_dir().join(format!("clio-inspect-tamper-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let opts = McpOpenOptions {
        backend: "sqlite".into(),
        database_url: dir.join("clio.db").to_string_lossy().into_owned(),
        kms_path: Some(dir.join("first.kms.json")),
        ..McpOpenOptions::default()
    };
    let first = McpState::open(&opts).expect("first state");
    store_item(&first, "itm-no-dek", "subj-no-dek", Some("lost one"));
    store_item(&first, "itm-tampered", "subj-tampered", Some("lost two"));
    store_item(
        &first,
        "itm-unopenable",
        "subj-unopenable",
        Some("lost three"),
    );
    drop(first);

    // Second state: same database, different (fresh) key store. Minting a new
    // DEK for `subj-tampered` turns its old envelope into an AEAD
    // authentication failure instead of a missing-DEK error.
    let second_opts = McpOpenOptions {
        kms_path: Some(dir.join("second.kms.json")),
        ..opts
    };
    let second = McpState::open(&second_opts).expect("second state");
    second
        .kms
        .encrypt("subj-tampered", b"mint a new DEK")
        .expect("mint dek");

    let out = call(&second, "inspect", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "listing must survive: {out}");
    let items = out["items"].as_array().expect("items");
    assert_eq!(items.len(), 3, "all metadata rows must still list: {out}");
    for id in ["itm-no-dek", "itm-tampered", "itm-unopenable"] {
        assert!(
            row(items, id).get("context").is_none(),
            "unreadable body on {id} must omit context: {out}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn context_schema_parity_across_store_admit_canonical_shared_and_correct() {
    let pack = crate::schema::schema_pack();
    let tools = pack["tools"].as_array().expect("tools");
    for name in [
        "store",
        "admit_preview",
        "canonical_put",
        "shared_store",
        "correct",
    ] {
        let tool = tools
            .iter()
            .find(|t| t["name"] == json!(name))
            .unwrap_or_else(|| panic!("tool `{name}` missing from the pack"));
        let prop = &tool["inputSchema"]["properties"]["context"];
        assert_eq!(prop["type"], json!("string"), "{name}: {prop}");
        assert_eq!(prop["maxLength"], json!(4096), "{name}: {prop}");
        assert!(
            !tool["inputSchema"]["required"]
                .as_array()
                .expect("required")
                .contains(&json!("context")),
            "{name} must not require context"
        );
    }
}

#[test]
fn correct_schema_requires_reason_but_not_new_value() {
    // The published schema must match the runtime gate. A belief
    // context-only correction omits `new_value`, so the schema must not list
    // it as required; item/attribute corrections still supply it (documented),
    // and the description must state the belief rejection rather than that
    // new_value is "ignored for belief targets".
    let pack = crate::schema::schema_pack();
    let tool = pack["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == json!("correct"))
        .expect("correct tool in the pack")
        .clone();
    let required = tool["inputSchema"]["required"]
        .as_array()
        .expect("required");
    assert_eq!(required, &vec![json!("item_id"), json!("reason")]);
    let description = tool["inputSchema"]["properties"]["new_value"]["description"]
        .as_str()
        .expect("new_value description");
    assert!(
        description.contains("MUST be omitted for belief"),
        "the description must tell belief callers to omit new_value: {description}"
    );
    assert!(
        !description.contains("ignored for belief"),
        "the schema must not say new_value is ignored for belief targets: {description}"
    );
}

#[test]
fn admit_preview_batch_candidate_schema_inherits_context() {
    let pack = crate::schema::schema_pack();
    let tool = pack["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == json!("admit_preview_batch"))
        .expect("admit_preview_batch");
    let candidate = &tool["inputSchema"]["properties"]["items"]["items"];
    let prop = &candidate["properties"]["context"];
    assert_eq!(prop["type"], json!("string"), "{prop}");
    assert_eq!(prop["maxLength"], json!(4096), "{prop}");
}
