// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the `inspect` supersession
//! section (see `read_inspect_supersession`).
//!
//! ## Owns
//! - Test coverage for the supersession report shape, pair rule, and verdicts
//! - The pure ordering-verdict tests over synthetic in-memory pages, and the
//!   twin-equivalence test proving the section's page matches the caller's
//!
//! ## Does not own
//! - The supersession section implementation (`read_inspect_supersession`)
//!
//! ## Boundary
//!
//! Changes to the supersession tests should normally require changes here.
//! Changes to the section implementation belong in `read_inspect_supersession`.
//!
//! Keep this module focused on the supersession section's tests.
//!
//! Fixture finding (measured, 2026-10-07): every write that sets
//! `superseded_by_id` also closes the prior row (`tx_until`), and both search
//! legs index only current-belief rows. The superseded member of a pair can
//! therefore never appear on a retrieval page, so the honest fixture produces
//! `not_on_page` (superseded absent, or both absent) and `no_link` — never
//! `replacement_first` or `superseded_first`. These tests pin the reachable
//! outcomes and never soften the fixture to manufacture the unreachable ones.

use super::{PageHit, Pair, ordering, page_report, page_request, retrieve_page};
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_retrieve::RetrieveOutcome;
use clio_store::CorrectionSpec;
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
fn absent_from(report: &Value, keys: &[&str]) {
    for key in keys {
        assert!(report.get(key).is_none(), "{key}: {report}");
    }
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
fn fillers(state: &McpState, bank: &str, n: usize) {
    for i in 0..n {
        let id = format!("ss-fill-{i}");
        let gist = format!("cadence cadence cadence report filler {i}");
        create(state, &item(&id, bank, &gist, TS_NEW), "create filler");
        index_doc(state, bank, &id, &gist, "filler lexical doc");
    }
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

#[test]
fn supersession_report_query_like_the_old_fact_is_the_named_failing_case() {
    let s = state();
    corrected_pair(&s, "bank-a");
    let report = supersession(&s, "bank-a", "ss-new", "deploy cadence nightly", None);
    assert_eq!(report["verdict"], json!("not_on_page"), "{report}");
    assert_eq!(report["query_item_id"], json!("ss-new"), "{report}");
    assert_eq!(report["replacement_item_id"], json!("ss-new"), "{report}");
    assert_eq!(report["superseded_item_id"], json!("ss-old"), "{report}");
    assert_eq!(report["missing"], json!(["superseded"]), "{report}");
    let reason = report["reason"].as_str().expect("reason").to_owned();
    assert!(
        reason.contains("fewer hits than the requested limit"),
        "{reason}"
    );
    assert_eq!(report["page_size"], json!(1), "{report}");
    assert_eq!(
        report["reads"],
        json!(["audit_trail", "retrieve"]),
        "{report}"
    );
    absent_from(&report, &["replacement_position", "superseded_position"]);
    absent_from(&report, &["replacement_score", "superseded_score"]);
    absent_from(&report, &["gist", "snapshot", "context", "subject_id"]);
    let prior = dispatch(&s, "get", &json!({"bank": "bank-a", "item_id": "ss-old"}));
    assert_eq!(prior["tx_until"], json!(TS_NEW), "{prior}");
}
#[test]
fn supersession_report_not_on_page_names_which_and_why() {
    let s = state();
    corrected_pair(&s, "bank-a");
    let report = supersession(&s, "bank-a", "ss-new", "quantum origami", None);
    assert_eq!(report["verdict"], json!("not_on_page"), "{report}");
    assert_eq!(
        report["missing"],
        json!(["replacement", "superseded"]),
        "{report}"
    );
    assert_eq!(report["page_size"], json!(0), "{report}");
    let reason = report["reason"].as_str().expect("reason").to_owned();
    assert!(
        reason.contains("fewer hits than the requested limit"),
        "{reason}"
    );
}
#[test]
fn supersession_report_full_page_reports_the_below_cut_reason() {
    let s = state();
    corrected_pair(&s, "bank-a");
    fillers(&s, "bank-a", 4);
    let report = supersession(&s, "bank-a", "ss-new", "cadence", Some(2));
    assert_eq!(report["verdict"], json!("not_on_page"), "{report}");
    assert_eq!(report["page_limit"], json!(2), "{report}");
    assert_eq!(report["page_size"], json!(2), "{report}");
    assert_eq!(
        report["missing"],
        json!(["replacement", "superseded"]),
        "{report}"
    );
    let reason = report["reason"].as_str().expect("reason").to_owned();
    assert!(reason.contains("cannot be distinguished"), "{reason}");
    assert!(reason.contains("below the page cut"), "{reason}");
}
#[test]
fn supersession_report_no_link_is_not_a_failing_case() {
    let s = state();
    corrected_pair(&s, "bank-a");
    create(
        &s,
        &item("ss-lone", "bank-a", "unrelated lone note", TS_OLD),
        "create lone",
    );
    let report = supersession(&s, "bank-a", "ss-lone", "deploy cadence nightly", None);
    assert_eq!(report["verdict"], json!("no_link"), "{report}");
    assert_eq!(report["query_item_id"], json!("ss-lone"), "{report}");
    assert_eq!(report["page_size"], json!(0), "{report}");
    assert_eq!(report["page_limit"], json!(0), "{report}");
    assert_eq!(report["reads"], json!(["audit_trail"]), "{report}");
    absent_from(&report, &["missing", "reason"]);
    absent_from(&report, &["replacement_item_id", "superseded_item_id"]);
    absent_from(&report, &["gist", "snapshot", "context", "subject_id"]);
}
#[test]
fn supersession_pair_flips_when_the_query_names_the_superseded_item() {
    let s = state();
    corrected_pair(&s, "bank-a");
    let report = supersession(&s, "bank-a", "ss-old", "deploy cadence weekly", None);
    assert_eq!(report["verdict"], json!("not_on_page"), "{report}");
    assert_eq!(report["query_item_id"], json!("ss-old"), "{report}");
    assert_eq!(report["replacement_item_id"], json!("ss-new"), "{report}");
    assert_eq!(report["superseded_item_id"], json!("ss-old"), "{report}");
    assert_eq!(report["missing"], json!(["superseded"]), "{report}");
}
#[test]
fn supersession_belief_target_has_no_revisions_and_reports_no_link() {
    let s = state();
    let args = json!({"proposition": "deploy cadence weekly", "confidence": 0.4, "evidence_ref": "ev-1", "source_type": "user_stated", "bank": "bank-a"});
    let create = dispatch(&s, "belief_observe", &args);
    assert_eq!(create["ok"], json!(true), "{create}");
    let belief_id = create["belief"]["id"].as_str().expect("belief id");
    let trail = clio_compliance::audit_trail(&*s.store, "bank-a", belief_id).expect("trail");
    assert!(trail.revisions.is_empty(), "{:?}", trail.revisions);
    let report = supersession(&s, "bank-a", belief_id, "deploy cadence", None);
    assert_eq!(report["verdict"], json!("no_link"), "{report}");
    assert_eq!(report["reads"], json!(["audit_trail"]), "{report}");
}

// Named ordering case at the report layer: two-member page names verdict + four fields.
#[test]
fn page_report_emits_ordering_fields_for_a_two_member_page() {
    let s = state();
    corrected_pair(&s, "bank-a");
    let ctx = s.resolve_ctx(&json!({"bank": "bank-a"})).expect("ctx");
    let args = json!({"bank": "bank-a"});
    let (_, mut out) = retrieve_page(&s, &ctx, &args, "deploy cadence nightly").expect("page");
    assert_eq!(out.hits.len(), 1, "{out:?}");
    assert_eq!(out.hits[0].hit.item_id, "ss-new", "{out:?}");
    out.hits[0].score = 0.9;
    out.hits[0].scores.final_score = 0.9;
    let mut second = out.hits[0].clone();
    second.hit.item_id = "ss-old".into();
    second.score = 0.7;
    second.scores.final_score = 0.7;
    out.hits.push(second);
    let pair = Pair {
        replacement_id: "ss-new".into(),
        superseded_id: "ss-old".into(),
    };
    let r = page_report(&pair, "ss-new", 10, &out);
    assert_eq!(r["verdict"], json!("replacement_first"), "{r}");
    for k in [
        "replacement_position",
        "superseded_position",
        "replacement_score",
        "superseded_score",
    ] {
        assert!(r.get(k).is_some(), "{k}: {r}");
    }
    let rep_pos = r["replacement_position"].as_u64().expect("rep-pos");
    let sup_pos = r["superseded_position"].as_u64().expect("sup-pos");
    assert!(rep_pos < sup_pos, "{r}");
    let rep_val = r["replacement_score"].as_f64().expect("rep-val");
    let sup_val = r["superseded_score"].as_f64().expect("sup-val");
    assert!(rep_val > sup_val, "{r}");
    absent_from(&r, &["missing", "reason"]);
}
fn hit(id: &str, score: f64) -> PageHit {
    PageHit {
        item_id: id.into(),
        score,
    }
}
// Pure ordering verdicts: score values are inputs only, asserts pin order never literals.
#[test]
fn supersession_ordering_replacement_first_names_both_members() {
    let hits = vec![hit("ss-new", 0.9), hit("ss-old", 0.7)];
    let ord = ordering(&hits, "ss-new", "ss-old");
    assert_eq!(ord.verdict.as_str(), "replacement_first", "{ord:?}");
    assert_eq!(ord.replacement.map(|(p, _)| p), Some(0), "{ord:?}");
    assert_eq!(ord.superseded.map(|(p, _)| p), Some(1), "{ord:?}");
    let (r, s) = (
        ord.replacement.map(|(_, v)| v),
        ord.superseded.map(|(_, v)| v),
    );
    assert!(r.is_some() && s.is_some() && r > s, "{ord:?}");
    assert!(ord.missing.is_empty(), "{ord:?}");
}
#[test]
fn supersession_ordering_superseded_first_names_both_members() {
    let hits = vec![hit("ss-old", 0.8), hit("ss-new", 0.6)];
    let ord = ordering(&hits, "ss-new", "ss-old");
    assert_eq!(ord.verdict.as_str(), "superseded_first", "{ord:?}");
    assert_eq!(ord.replacement.map(|(p, _)| p), Some(1), "{ord:?}");
    assert_eq!(ord.superseded.map(|(p, _)| p), Some(0), "{ord:?}");
    let (r, s) = (
        ord.replacement.map(|(_, v)| v),
        ord.superseded.map(|(_, v)| v),
    );
    assert!(r.is_some() && s.is_some() && r < s, "{ord:?}");
    assert!(ord.missing.is_empty(), "{ord:?}");
}
#[test]
fn supersession_ordering_not_on_page_names_which_member_is_missing() {
    let one_hit = vec![hit("ss-new", 0.9)];
    let ord = ordering(&one_hit, "ss-new", "ss-old");
    assert_eq!(ord.verdict.as_str(), "not_on_page", "{ord:?}");
    assert_eq!(ord.replacement.map(|(p, _)| p), Some(0), "{ord:?}");
    assert!(ord.superseded.is_none(), "{ord:?}");
    assert_eq!(ord.missing, vec!["superseded"], "{ord:?}");
    let empty: Vec<PageHit> = Vec::new();
    let both = ordering(&empty, "ss-new", "ss-old");
    assert_eq!(both.verdict.as_str(), "not_on_page", "{both:?}");
    assert!(both.replacement.is_none(), "{both:?}");
    assert!(both.superseded.is_none(), "{both:?}");
    assert_eq!(both.missing, vec!["replacement", "superseded"], "{both:?}");
}
#[test]
fn supersession_twin_page_matches_the_shared_retriever_page() {
    let s = state();
    corrected_pair(&s, "bank-a");
    let ctx = s.resolve_ctx(&json!({"bank": "bank-a"})).expect("ctx");
    let args = json!({"bank": "bank-a"});
    let request = page_request(&s, &ctx, &args, "deploy cadence nightly").expect("request");
    let (_, twin_page) = retrieve_page(&s, &ctx, &args, "deploy cadence nightly").expect("twin");
    let shared_page = s.retriever.retrieve(&request).expect("shared page");
    let ids = |o: &RetrieveOutcome| {
        o.hits
            .iter()
            .map(|h| h.hit.item_id.clone())
            .collect::<Vec<_>>()
    };
    assert!(
        !twin_page.hits.is_empty(),
        "the fixture must produce a non-empty page"
    );
    assert_eq!(ids(&twin_page), ids(&shared_page));
}
