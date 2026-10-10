// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the supersession read-only witness tests:
//! the `inspect` supersession section reaches no mutating `Store` method,
//! starts no maintenance wave, and names only read methods in its `reads`
//! report, with the counting double proven live against a known write.
//!
//! ## Owns
//! - The zero-write witness at one and two hits, and the measured evidence
//!   that the shared retriever still reinforces a two-hit page (the section
//!   path itself runs through the read-only diagnostic twin)
//!
//! ## Does not own
//! - The counting double and its writer names (`store_double`)
//! - The double installation and the seeded state (`main`)
//! - The supersession section itself (`clio-mcp` read dispatch)
//!
//! ## Boundary
//!
//! Changes to what a supersession read reaches should normally require
//! changes here. Changes to the report shape should not.
//!
//! Keep this module focused on witnessing the supersession read path.

use crate::store_double::{CountingStore, WRITER_NAMES};
use crate::support;
use crate::{seeded_state, with_double};
use clio_mcp::runtime::McpState;
use clio_mcp::write_tools::dispatch;
use clio_store::CorrectionSpec;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::sync::Arc;

const TS_OLD: &str = "2026-01-01T00:00:00Z";
const TS_NEW: &str = "2026-02-01T00:00:00Z";
const SUPERSEDED: &str = "ssw-old";
const REPLACEMENT: &str = "ssw-new";

/// One synthetic semantic fact; no real content anywhere in the fixture.
fn pair_item(id: &str, gist: &str, ts: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: support::BANK.into(),
        subject_id: "subject".into(),
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

/// Seed the synthetic corrected pair through the real store BEFORE the double
/// is installed, so the setup writes never touch the counter. The superseded
/// revision's gist matches the old-fact query better than the replacement's.
fn seed_pair(state: &McpState) {
    let prior = pair_item(SUPERSEDED, "deploy cadence nightly checks", TS_OLD);
    state
        .store
        .create_memory_item(&prior, "agent")
        .expect("seed prior");
    state
        .index
        .put_lexical_doc(support::BANK, SUPERSEDED, "deploy cadence nightly checks")
        .expect("seed prior doc");
    let mut corrected = prior;
    corrected.id = REPLACEMENT.into();
    corrected.gist = Some("deploy cadence weekly checks".into());
    corrected.snapshot = Some(json!({"cadence": "weekly"}));
    corrected.valid_from = TS_NEW.into();
    corrected.tx_from = TS_NEW.into();
    corrected.created_at = TS_NEW.into();
    corrected.updated_at = TS_NEW.into();
    state
        .store
        .correct_item(&CorrectionSpec {
            bank_id: support::BANK.into(),
            prior_item_id: SUPERSEDED.into(),
            new_item: corrected,
            reason: "cadence changed".into(),
            actor: "agent".into(),
            timestamp: TS_NEW.into(),
        })
        .expect("seed correct");
    state
        .index
        .put_lexical_doc(support::BANK, REPLACEMENT, "deploy cadence weekly checks")
        .expect("seed replacement doc");
}

/// The supersession section for one query, through the inspect binding.
fn inspect_supersession(state: &McpState, item_id: &str, query: &str) -> Value {
    let out = dispatch(
        state,
        "inspect",
        &json!({"bank": support::BANK, "item_id": item_id, "query": query}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    out["supersession"].clone()
}

/// The read-only witness. The double is proven live first: one known write
/// through it moves the counter, so a counting-nothing double cannot satisfy
/// the zero assertions vacuously. The supersession path then runs through the
/// double-backed dispatch with a one-hit page — the section reads through the
/// read-only diagnostic twin — and the mutation counter, the view counter, and
/// the wave counter must not move.
#[test]
fn supersession_report_reaches_no_writer_and_the_double_counts_a_known_write() {
    let seeded = seeded_state();
    seed_pair(&seeded);
    let double = Arc::new(CountingStore::new(Arc::clone(&seeded.store)));
    let state = with_double(&seeded, &double);

    let before = double.mutations();
    state
        .store
        .record_audit_event(
            support::BANK,
            "witness_live",
            None,
            "agent",
            &json!({"probe": true}),
        )
        .expect("known write through the double");
    assert_eq!(
        double.mutations(),
        before + 1,
        "the counting double must count a real write"
    );
    let live = double.mutations();

    let waves_before = state.maint.waves_started();
    let report = inspect_supersession(&state, REPLACEMENT, "deploy cadence nightly");

    assert_eq!(report["verdict"], json!("not_on_page"), "{report}");
    let reads = report["reads"].as_array().expect("reads list");
    assert!(
        !reads.is_empty(),
        "the report must name the methods it reached"
    );
    for name in reads {
        let method = name.as_str().expect("read method name");
        assert!(
            !WRITER_NAMES.contains(&method),
            "{method} must not be a documented writer"
        );
    }
    assert_eq!(
        double.mutations(),
        live,
        "the supersession read must not reach a mutating store method: {report}"
    );
    assert_eq!(
        double.view_writes(),
        0,
        "no writer behind any wrapped view handle: {report}"
    );
    assert_eq!(
        state.maint.waves_started(),
        waves_before,
        "no maintenance wave may start: {report}"
    );
}

/// The read-only contract at the interesting page size: through the diagnostic
/// twin, the section's two-hit page performs zero mutations, while the same
/// two-hit page through the shared retriever still reaches the co-activation
/// handoff writers. Measured, not assumed: one reinforced pair plus one audit
/// row.
#[test]
fn a_two_hit_supersession_page_is_zero_write_through_the_twin() {
    let seeded = seeded_state();
    seed_pair(&seeded);
    let filler_id = "ssw-fill-0";
    let filler_gist = "deploy cadence checklist zero";
    let filler = pair_item(filler_id, filler_gist, TS_NEW);
    seeded
        .store
        .create_memory_item(&filler, "agent")
        .expect("seed filler");
    seeded
        .index
        .put_lexical_doc(support::BANK, filler_id, filler_gist)
        .expect("seed filler doc");

    let double = Arc::new(CountingStore::new(Arc::clone(&seeded.store)));
    let state = with_double(&seeded, &double);

    let before = double.mutations();
    let report = inspect_supersession(&state, REPLACEMENT, "deploy cadence");

    assert_eq!(report["page_size"], json!(2), "{report}");
    assert_eq!(
        double.mutations(),
        before,
        "the section's two-hit page must reach no writer through the twin: {report}"
    );
    assert_eq!(
        double.view_writes(),
        0,
        "no writer behind any wrapped view handle: {report}"
    );

    // Evidence of the pre-existing retrieve behavior the twin replaces on the
    // report's path only: the shared retriever, called directly through the
    // `retrieve` tool on the same page size, still writes.
    let evidence = dispatch(
        &state,
        "retrieve",
        &json!({"bank": support::BANK, "query": "deploy cadence"}),
    );
    assert_eq!(evidence["ok"], json!(true), "{evidence}");
    assert_eq!(evidence["pairs_reinforced"], json!(1), "{evidence}");
    assert_eq!(
        double.mutations(),
        before + 2,
        "a two-hit page must still reach the co-activation handoff writers \
         through the shared retriever (one reinforce + one audit): {evidence}"
    );
}
