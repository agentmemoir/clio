// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the guard, refusal, and visibility tests
//! of the `inspect` supersession section (see `read_inspect_supersession`).
//!
//! ## Owns
//! - The refusal tests for a missing `query` and a foreign-bank `item_id`
//! - The determinism, dense-visibility, and inspect-payload-shape tests
//! - The score-guard test proving no test pins a supersession score value
//!
//! ## Does not own
//! - The supersession section implementation (`read_inspect_supersession`)
//! - The report-shape and pure ordering-verdict tests
//!   (`read_inspect_supersession_tests`)
//!
//! ## Boundary
//!
//! Changes to the guard, refusal, or visibility tests should normally
//! require changes here. Changes to the section implementation belong in
//! `read_inspect_supersession`.
//!
//! Keep this module focused on the supersession section's guard tests.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_store::{CorrectionSpec, DEFAULT_EMBEDDING_MODEL, EMBEDDING_DIMS, VectorSlot};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};

fn state() -> std::sync::Arc<McpState> {
    let opts = McpOpenOptions {
        default_bank: "bank-a".into(),
        ..McpOpenOptions::default()
    };
    McpState::open_with_effective(&opts, &json!({})).expect("state")
}
const TS_OLD: &str = "2026-01-01T00:00:00Z";
const TS_NEW: &str = "2026-02-01T00:00:00Z";
fn item(id: &str, bank: &str, gist: &str, ts: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.8),
        source_ref: Some("fixture".into()),
        snapshot: Some(json!({"entity": id})),
        gist: Some(gist.into()),
        valid_from: ts.into(),
        valid_until: None,
        tx_from: ts.into(),
        tx_until: None,
        created_at: ts.into(),
        updated_at: ts.into(),
        context: None,
    }
}
fn create(s: &McpState, row: &MemoryItem, what: &str) {
    s.store.create_memory_item(row, "agent").expect(what);
}
fn index_doc(s: &McpState, bank: &str, id: &str, text: &str, what: &str) {
    s.index.put_lexical_doc(bank, id, text).expect(what);
}
fn corrected_pair(state: &McpState, bank: &str) {
    let old_text = "deploy cadence nightly checks";
    let new_text = "deploy cadence weekly checks";
    let prior = item("ss-old", bank, old_text, TS_OLD);
    create(state, &prior, "create prior");
    index_doc(state, bank, "ss-old", old_text, "prior");
    let mut corrected = prior;
    corrected.id = "ss-new".into();
    corrected.gist = Some("deploy cadence weekly checks".into());
    corrected.snapshot = Some(json!({"cadence": "weekly"}));
    corrected.valid_from = TS_NEW.into();
    corrected.tx_from = TS_NEW.into();
    corrected.created_at = TS_NEW.into();
    corrected.updated_at = TS_NEW.into();
    let spec = CorrectionSpec {
        bank_id: bank.into(),
        prior_item_id: "ss-old".into(),
        new_item: corrected,
        reason: "cadence changed".into(),
        actor: "agent".into(),
        timestamp: TS_NEW.into(),
    };
    state.store.correct_item(&spec).expect("correct prior");
    index_doc(state, bank, "ss-new", new_text, "replacement");
}

fn supersession(s: &McpState, bank: &str, id: &str, query: &str, limit: Option<usize>) -> Value {
    let mut args = json!({"bank": bank, "item_id": id, "query": query});
    if let Some(n) = limit {
        args["limit"] = json!(n);
    }
    let out = dispatch(s, "inspect", &args);
    assert_eq!(out["ok"], json!(true), "{out}");
    out["supersession"].clone()
}

// One-hot vector slot; the pattern follows `index_store_tests::slot`.
fn slot(bank: &str, id: &str, axis: usize) -> VectorSlot {
    let mut embedding = vec![0.0_f32; EMBEDDING_DIMS];
    embedding[axis % EMBEDDING_DIMS] = 1.0;
    VectorSlot {
        item_id: id.into(),
        bank_id: bank.into(),
        model_id: DEFAULT_EMBEDDING_MODEL.into(),
        embedding,
    }
}

// Refusals: a missing `query` and a foreign-bank `item_id` fail with codes.
#[test]
fn supersession_section_refuses_a_missing_query_and_a_foreign_bank_item() {
    let s = state();
    corrected_pair(&s, "bank-a");
    let out = dispatch(
        &s,
        "inspect",
        &json!({"bank": "bank-a", "item_id": "ss-new"}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"), "{out}");
    assert_eq!(out["message"], json!("query is required"), "{out}");
    let args = json!({"bank": "bank-b", "item_id": "ss-new", "query": "deploy cadence"});
    let foreign = dispatch(&s, "inspect", &args);
    assert_eq!(foreign["ok"], json!(false), "{foreign}");
    assert_eq!(foreign["code"], json!("item_not_found"), "{foreign}");
}
#[test]
fn supersession_report_is_deterministic_same_query_twice() {
    let s = state();
    corrected_pair(&s, "bank-a");
    let first = supersession(&s, "bank-a", "ss-new", "deploy cadence nightly", None);
    let second = supersession(&s, "bank-a", "ss-new", "deploy cadence nightly", None);
    assert_eq!(first, second, "{first}\n{second}");
}

// Dense re-measurement: the closed row's vector survives raw KNN but filtered dense search hides it.
#[test]
fn dense_leg_hides_the_closed_row_that_raw_knn_still_returns() {
    let s = state();
    corrected_pair(&s, "bank-a");
    s.store
        .upsert_vector(&slot("bank-a", "ss-old", 0))
        .expect("old vector");
    s.store
        .upsert_vector(&slot("bank-a", "ss-new", 1))
        .expect("new vector");
    let probe = slot("bank-a", "ss-old", 0).embedding;
    let raw = s.store.knn("bank-a", &probe, 5).expect("knn");
    assert!(
        raw.iter().any(|h| h.item_id == "ss-old"),
        "stale vector residue: {raw:?}"
    );
    let dense = s
        .index
        .dense_search("bank-a", &probe, &[], 5)
        .expect("dense");
    assert!(
        !dense.iter().any(|h| h.item_id == "ss-old"),
        "closed row filtered: {dense:?}"
    );
    assert!(
        dense.iter().any(|h| h.item_id == "ss-new"),
        "live row visible: {dense:?}"
    );
    let report = supersession(&s, "bank-a", "ss-new", "deploy cadence nightly", None);
    assert_eq!(report["verdict"], json!("not_on_page"), "{report}");
}

#[test]
fn inspect_without_item_id_keeps_existing_fields_byte_identical() {
    let s = state();
    corrected_pair(&s, "bank-a");
    let plain = dispatch(&s, "inspect", &json!({"bank": "bank-a"}));
    assert_eq!(plain["ok"], json!(true), "{plain}");
    let obj = plain.as_object().expect("payload object");
    let mut keys: Vec<String> = obj.keys().cloned().collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "include_discarded",
            "items",
            "limit",
            "offset",
            "ok",
            "total"
        ],
        "{keys:?}"
    );
    assert!(plain.get("supersession").is_none(), "{plain}");
    let mut named = json!({"bank": "bank-a", "item_id": "ss-new"});
    named["query"] = json!("deploy cadence nightly");
    let with_section = dispatch(&s, "inspect", &named);
    assert_eq!(with_section["ok"], json!(true), "{with_section}");
    let mut stripped = with_section;
    stripped
        .as_object_mut()
        .expect("payload object")
        .remove("supersession");
    assert_eq!(stripped, plain, "the section must add only its own key");
}

// No test pins a score: assert lines with float literals or score/position comparisons fail.
#[test]
fn no_supersession_score_value_is_pinned_in_the_test_sources() {
    const COMPARE: [&str; 2] = ["assert_eq!", "== "];
    let has_float = |l: &str| {
        l.as_bytes()
            .windows(3)
            .any(|w| w[0].is_ascii_digit() && w[1] == b'.' && w[2].is_ascii_digit())
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for f in [
        "src/read_inspect_supersession_tests.rs",
        "src/read_inspect_supersession_guard_tests.rs",
        "tests/pressure/supersession_zero_write.rs",
    ] {
        let src = std::fs::read_to_string(root.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
        for (n, line) in src.lines().enumerate() {
            if line.trim_start().starts_with("//") || !line.contains("assert") {
                continue;
            }
            assert!(!has_float(line), "{f}:{} pins a value: {line}", n + 1);
            let low = line.to_ascii_lowercase();
            if low.contains("score") || low.contains("position") {
                for bad in COMPARE {
                    assert!(!line.contains(bad), "{f}:{} compares value: {line}", n + 1);
                }
            }
        }
    }
}
