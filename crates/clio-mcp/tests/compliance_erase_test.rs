// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! End-to-end compliance erasure conformance.
//!
//! Ingest items → read them → erase → DEK destroyed → reads fail closed with
//! `erased_subject` → tombstones are content-free.
//!
//! A state here names its own scratch journal path through the document seam.
//! That path diverges from the deployment resolution, so the MCP `erase_request`
//! tool refuses fail-closed; the erasure a downstream surface needs is driven
//! through the compliance handler directly, and one test pins the refusal.

use clio_compliance::{EraseArgs, EraseVecSink, erase_request, tombstone_salt};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use clio_types::{Actor, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const BANK: &str = "erase-e2e-bank";
const SUBJECT: &str = "erase-e2e-subject";

/// Calls served so far, so no two tests share a scratch directory.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One per-test scratch directory for the erasure journal, removed on drop.
///
/// The store is in-memory, but the journal is a real owner-only file: without
/// its own directory the confirmed erasure would write the deployment's
/// resolved journal path and a parallel test would race it.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "am-compliance-erase-{tag}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self { dir }
    }

    fn journal(&self) -> PathBuf {
        self.dir.join("erasure-journal.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn state(scratch: &Scratch) -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            shared_bank_enabled: false,
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: BANK.into(),
            default_actor: Actor::Harness,
            kms_path: None,
        },
        &json!({ "erasure_journal_file": scratch.journal().to_string_lossy() }),
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

/// Destroy a subject's DEK through the real compliance erasure call.
///
/// A downstream surface's setup uses this directly rather than the MCP
/// `erase_request` tool: the tool refuses a document-named journal that
/// diverges from the deployment resolution, while this test is about retrieve
/// after an erasure, not about the erase tool.
fn erase_directly(st: &McpState, journal: &Path, subject: &str, request: &str) {
    let sink = EraseVecSink::default();
    let ctx = OpContext::new(Bank::new(BANK), Actor::Harness);
    let salt = tombstone_salt();
    let outcome = erase_request(&EraseArgs {
        store: &*st.compliance,
        journal_path: journal,
        maint: Some(&st.maint),
        ctx: &ctx,
        subject_id: subject,
        legal_basis: "GDPR_ART_17",
        request_id: request,
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: "2024-01-01T00:00:00Z",
        sink: &sink,
    })
    .expect("run the erasure");
    assert_eq!(outcome.status, "COMPLETED", "the erasure did not complete");
}

/// A document-named journal that differs from the deployment resolution is
/// refused: erase would otherwise lock a file the restore CLI never reads, so
/// the erase denial and restore placement would not share one lock.
#[test]
fn erase_request_refuses_a_document_divergent_from_the_deployment() {
    let scratch = Scratch::new("divergent");
    let st = state(&scratch);
    let item_id = "e2e-item-divergent";
    st.store
        .create_memory_item(&item(item_id), "harness")
        .expect("seed item");

    let refused = dispatch(
        &st,
        "erase_request",
        &erase_args("harness", true, "req-divergent"),
    );
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["code"], "config_corrupt", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or_default()
            .contains("CLIO_ERASURE_JOURNAL_FILE"),
        "the refusal must name the remedy: {refused}"
    );

    // Nothing was shredded, no denial was written, and the item stays readable.
    assert!(
        !st.compliance.subject_dek_destroyed(SUBJECT).unwrap(),
        "a refused erase must not destroy the subject key"
    );
    assert!(
        !scratch.journal().exists(),
        "a refused erase must not write the divergent journal"
    );
    let read = dispatch(
        &st,
        "get_snapshot",
        &json!({"bank": BANK, "item_id": item_id}),
    );
    assert_eq!(read["ok"], true, "the item stays readable: {read}");
}

// Retrieve must never return a crypto-shredded item in candidates.
#[test]
fn retrieve_excludes_erased_item_from_candidates() {
    let scratch = Scratch::new("retrieve");
    let st = state(&scratch);
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

    erase_directly(&st, &scratch.journal(), SUBJECT, "req-t19-03");

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
