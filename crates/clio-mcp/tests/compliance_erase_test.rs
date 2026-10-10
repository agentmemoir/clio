// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! End-to-end compliance erasure conformance.
//!
//! Ingest items → read them → `erase_request` → DEK destroyed → reads fail
//! closed with `erased_subject` → tombstones are content-free.

use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use clio_types::{Actor, EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::sync::Arc;

const BANK: &str = "erase-e2e-bank";
const SUBJECT: &str = "erase-e2e-subject";

fn state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            shared_bank_enabled: false,
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: BANK.into(),
            default_actor: Actor::Harness,
            kms_path: None,
        },
        &serde_json::json!({}),
    )
    .expect("state")
}

fn item(id: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: SUBJECT.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("persona").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(json!({"entity": "private-value"})),
        gist: Some("private gist".into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: None,
    }
}

fn erase_args(actor: &str, confirm: bool, request: &str) -> Value {
    json!({
        "bank": BANK,
        "actor": actor,
        "subject_id": SUBJECT,
        "legal_basis": "GDPR_ART_17",
        "request_id": request,
        "confirm": confirm
    })
}

#[test]
fn erase_request_end_to_end_fails_closed() {
    let st = state();
    let item_id = "e2e-item-1";
    st.store
        .create_memory_item(&item(item_id), "harness")
        .expect("seed item");

    // Readable before erasure.
    let before = dispatch(
        &st,
        "get_snapshot",
        &json!({"bank": BANK, "item_id": item_id}),
    );
    assert_eq!(before["ok"], true, "{before}");

    // Agent actor is not privileged: 403, zero writes.
    let agent = dispatch(&st, "erase_request", &erase_args("agent", true, "r-agent"));
    assert_eq!(agent["ok"], false);
    assert_eq!(agent["code"], "forbidden");
    assert!(!st.compliance.subject_dek_destroyed(SUBJECT).unwrap());

    // Harness without confirm is rejected with zero writes.
    let unconfirmed = dispatch(
        &st,
        "erase_request",
        &erase_args("harness", false, "r-noconfirm"),
    );
    assert_eq!(unconfirmed["ok"], false);
    assert_eq!(unconfirmed["code"], "forbidden");
    assert!(!st.compliance.subject_dek_destroyed(SUBJECT).unwrap());

    // Confirmed privileged erasure completes.
    let done = dispatch(
        &st,
        "erase_request",
        &erase_args("harness", true, "req-e2e-1"),
    );
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["status"], "COMPLETED");
    assert_eq!(done["request_id"], "req-e2e-1");
    assert_eq!(done["item_count"], 1);
    assert_eq!(done["subject_hash"].as_str().unwrap().len(), 64);

    // Fail-closed reads: no panic, structured erased_subject indicator.
    for tool in ["get", "get_snapshot", "get_gist"] {
        let read = dispatch(&st, tool, &json!({"bank": BANK, "item_id": item_id}));
        assert_eq!(read["ok"], false, "{tool}: {read}");
        assert_eq!(read["code"], "erased_subject", "{tool}: {read}");
    }

    // Tombstones are content-free.
    let tombstones = st.compliance.read_tombstones(BANK, "req-e2e-1").unwrap();
    assert_eq!(tombstones.len(), 1);
    let t = &tombstones[0];
    assert_eq!(t.subject_hash.len(), 64);
    assert_eq!(t.item_id_hash.as_deref().map(str::len), Some(64));
    assert_eq!(t.reason_code, "COMPLIANCE_CRYPTO_SHRED");
    let serialized = serde_json::to_string(&tombstones).unwrap();
    assert!(!serialized.contains(SUBJECT), "{serialized}");
    assert!(!serialized.contains(item_id), "{serialized}");
    assert!(!serialized.contains("private"), "{serialized}");

    // Erasing an unknown subject reports not_found and writes no tombstone.
    let missing = dispatch(
        &st,
        "erase_request",
        &json!({
            "bank": BANK, "actor": "harness", "subject_id": "missing-subject",
            "legal_basis": "GDPR_ART_17", "request_id": "req-missing", "confirm": true
        }),
    );
    assert_eq!(missing["ok"], false);
    assert_eq!(missing["code"], "not_found");
    let tombstones = st.compliance.read_tombstones(BANK, "req-missing").unwrap();
    assert!(tombstones.is_empty(), "{tombstones:?}");
}

// Retrieve must never return a crypto-shredded item in candidates.
#[test]
fn retrieve_excludes_erased_item_from_candidates() {
    let st = state();
    let item_id = "e2e-retrieve-1";
    let stored = dispatch(
        &st,
        "store",
        &json!({
            "item": {
                "id": item_id,
                "subject_id": SUBJECT,
                "gist": "private medical note",
                "snapshot": {"entity": "private medical note", "amount": 1, "date": "2024-01-01"}
            },
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": "The user prefers private medical note (1 note since 2024-01-01).",
            "bank": BANK,
            "actor": "agent",
            "timestamp": "2024-01-01T00:00:00Z"
        }),
    );
    assert_eq!(stored["ok"], true, "{stored}");
    st.index
        .put_lexical_doc(BANK, item_id, "private medical note")
        .expect("lexical doc");

    // Readable through candidate search before erasure.
    let before = dispatch(
        &st,
        "retrieve",
        &json!({"bank": BANK, "query": "private medical"}),
    );
    assert_eq!(before["ok"], true, "{before}");
    let found = before["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .any(|h| h["item_id"].as_str() == Some(item_id));
    assert!(found, "{before}");

    let done = dispatch(
        &st,
        "erase_request",
        &erase_args("harness", true, "req-t19-03"),
    );
    assert_eq!(done["ok"], true, "{done}");

    // After erasure the item is absent from candidate results, and no
    // personal text leaks through the retrieval surface.
    let after = dispatch(
        &st,
        "retrieve",
        &json!({"bank": BANK, "query": "private medical"}),
    );
    assert_eq!(after["ok"], true, "{after}");
    let serialized = serde_json::to_string(&after).unwrap();
    assert!(!serialized.contains(item_id), "{serialized}");
    assert!(!serialized.contains("private"), "{serialized}");
    assert!(!serialized.contains("medical"), "{serialized}");
}
