// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for compose-context tests: dual budgets, persona
//! channel emission, gate-skip behavior, and budget invariants across sizes.
//!
//! ## Owns
//! - Packer budget assertions for T-05b, including the never-exceeds-budget
//!   sweep and the underflow rejection
//!
//! ## Does not own
//! - Retrieval ordering (`hybrid_tests`) or tool-surface parsing (`surface_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::compose::{
    ComposePack, ComposeRequest, Composer, PACK_OVERHEAD_TOKENS, PackSection, PersonaEntry,
};
use crate::fixtures_tests as fx;
use crate::intent::{GateDecision, intent_gate};
use crate::pack_forms::{FormTally, HitForm, REFERENCE_MARKER};
use crate::token::estimate_tokens;
use clio_config::ranking::IntentGateThresholds;
use std::sync::Arc;

fn composer(store: &Arc<clio_store::SqliteStore>) -> Composer {
    Composer::new(Arc::new(fx::retriever(store)))
}

fn section<'a>(pack: &'a ComposePack, name: &str) -> &'a PackSection {
    pack.sections.iter().find(|s| s.name == name).expect(name)
}

fn request(bank: &str, query: Option<&str>, budget: usize) -> ComposeRequest {
    ComposeRequest {
        bank: bank.into(),
        query: query.map(str::to_owned),
        budget_tokens: budget,
        ..ComposeRequest::new(bank)
    }
}

fn entry(key: &str, text: &str, score: f64) -> PersonaEntry {
    PersonaEntry {
        key: key.into(),
        text: text.into(),
        score,
    }
}

fn skip_gate() -> GateDecision {
    GateDecision {
        retrieval_needed: false,
        domains: Vec::new(),
        confidence: 0.95,
        reason: "phatic".into(),
    }
}

#[test]
fn t11_oversize_bank_respects_budget() {
    let store = fx::store();
    for i in 0..8 {
        let item = fx::item(
            &format!("M{i}"),
            "bank",
            "shared token content padding padding",
        );
        fx::put_lexical(&store, &item, "shared token content padding padding");
    }
    let mut req = request("bank", Some("shared"), 60);
    req.persona_entries = vec![entry("style", "concise", 0.9)];
    let pack = composer(&store).compose(&req, None).expect("compose");
    assert!(pack.total_tokens <= 60, "total {}", pack.total_tokens);
    assert!(pack.retrieval_ran);
    let memory = section(&pack, "memory");
    assert!(memory.tokens <= pack.memory_budget_tokens);
    assert!(pack.sections.iter().any(|s| s.name == "persona"));
}

#[test]
fn t05_auto_path_skips_memory_but_keeps_persona() {
    let store = fx::store();
    let item = fx::item("A", "bank", "shared token");
    fx::put_lexical(&store, &item, "shared token");
    let mut req = request("bank", Some("shared"), 400);
    req.persona_entries = vec![entry("name", "Ada", 0.9)];
    let pack = composer(&store)
        .compose(&req, Some(&skip_gate()))
        .expect("compose");
    assert!(!pack.retrieval_ran);
    let persona = section(&pack, "persona");
    assert_eq!(persona.item_ids, vec!["name".to_owned()]);
    let memory = section(&pack, "memory");
    assert!(memory.item_ids.is_empty(), "{:?}", memory.item_ids);
    assert_eq!(memory.tokens, 0);
}

#[test]
fn t05b_explicit_compose_after_gate_skip_still_retrieves() {
    let store = fx::store();
    let item = fx::item("A", "bank", "city live");
    fx::put_lexical(&store, &item, "city live");
    let decision = intent_gate("hi", &IntentGateThresholds::default());
    assert!(!decision.retrieval_needed);
    // Explicit tool: gate argument omitted, so memory retrieval runs.
    let req = request("bank", Some("city"), 400);
    let pack = composer(&store).compose(&req, None).expect("compose");
    assert!(pack.retrieval_ran);
    let memory = section(&pack, "memory");
    assert_eq!(memory.item_ids, vec!["A".to_owned()]);
}

#[test]
fn persona_entries_rank_and_truncate_by_budget() {
    let store = fx::store();
    let mut req = request("bank", None, 200);
    req.persona_entries = vec![
        entry("low", "low value", 0.1),
        entry("high", "high value", 0.9),
        entry("mid", "mid value", 0.5),
    ];
    let pack = Composer::new(Arc::new(fx::retriever(&store)))
        .with_persona_budget(4)
        .compose(&req, None)
        .expect("compose");
    let persona = section(&pack, "persona");
    assert!(persona.tokens <= 4);
    assert_eq!(persona.item_ids.first().map(String::as_str), Some("high"));
    assert!(!pack.truncated_ids.is_empty(), "{:?}", pack.truncated_ids);
    assert!(persona.truncated);
}

#[test]
fn persona_score_ties_break_by_key() {
    let store = fx::store();
    let mut req = request("bank", None, 200);
    req.persona_entries = vec![entry("b", "x", 0.5), entry("a", "x", 0.5)];
    let pack = composer(&store).compose(&req, None).expect("compose");
    let persona = section(&pack, "persona");
    assert_eq!(persona.item_ids, vec!["a".to_owned(), "b".to_owned()]);
}

#[test]
fn empty_persona_entries_still_emit_section() {
    let store = fx::store();
    let req = request("bank", None, 100);
    let pack = composer(&store).compose(&req, None).expect("compose");
    let persona = section(&pack, "persona");
    assert_eq!(persona.tokens, 0);
    assert!(!persona.truncated);
    assert!(!pack.retrieval_ran);
}

#[test]
fn no_query_yields_persona_only() {
    let store = fx::store();
    let mut req = request("bank", None, 100);
    req.persona_entries = vec![entry("k", "v", 0.5)];
    let pack = composer(&store).compose(&req, None).expect("compose");
    assert!(!pack.retrieval_ran);
    assert!(pack.total_tokens <= 100);
}

#[test]
fn budget_underflow_is_rejected() {
    let store = fx::store();
    let composer = composer(&store);
    let too_small = request("bank", None, PACK_OVERHEAD_TOKENS);
    assert!(composer.compose(&too_small, None).is_err());
    assert!(composer.compose(&request("bank", None, 0), None).is_err());
}

#[test]
fn missing_bank_is_rejected() {
    let store = fx::store();
    assert!(
        composer(&store)
            .compose(&request("", None, 100), None)
            .is_err()
    );
}

// Every included hit contributes exactly one form, in id order: one form per id.
#[test]
fn forms_partition_included_ids() {
    let store = fx::store();
    let gist = fx::item("G", "bank", "shared gist");
    fx::put_lexical(&store, &gist, "shared gist");
    let mut bare = fx::item("B", "bank", "shared");
    bare.gist = None;
    bare.snapshot = None;
    fx::put_lexical(&store, &bare, "shared bare");
    let req = request("bank", Some("shared"), 400);
    let pack = composer(&store).compose(&req, None).expect("compose");
    let memory = section(&pack, "memory");
    assert_eq!(memory.hit_forms.len(), memory.item_ids.len());
    assert!(
        memory
            .hit_forms
            .iter()
            .all(|f| matches!(f, HitForm::Prose | HitForm::Exact | HitForm::Reference))
    );
    assert!(memory.hit_forms.contains(&HitForm::Prose));
    assert!(memory.hit_forms.contains(&HitForm::Reference));
}

// A gistless hit renders as a marked reference, never as a bare id: the
// marker is fixed, and these hits previously rendered bare ids.
#[test]
fn gistless_hits_render_as_marked_references() {
    let store = fx::store();
    let mut snapshot_only = fx::item("S", "bank", "shared");
    snapshot_only.gist = None;
    fx::put_lexical(&store, &snapshot_only, "shared snapshot");
    let mut bare = fx::item("B", "bank", "shared");
    bare.gist = None;
    bare.snapshot = None;
    fx::put_lexical(&store, &bare, "shared bare");
    let req = request("bank", Some("shared"), 400);
    let pack = composer(&store).compose(&req, None).expect("compose");
    let memory = section(&pack, "memory");
    let lines: Vec<&str> = memory.text.lines().collect();
    for id in ["S", "B"] {
        let marked = format!("{REFERENCE_MARKER}{id}");
        assert!(lines.contains(&marked.as_str()), "{lines:?}");
    }
    assert!(
        lines.iter().all(|l| l.starts_with(REFERENCE_MARKER)),
        "{lines:?}"
    );
    assert!(!lines.contains(&"S") && !lines.contains(&"B"));
}

// The pack tally is the memory record's fold, counted once, over included hits.
#[test]
fn form_tally_equals_record_fold() {
    let store = fx::store();
    let gist = fx::item("G", "bank", "shared gist");
    fx::put_lexical(&store, &gist, "shared gist");
    let mut snapshot_only = fx::item("S", "bank", "shared");
    snapshot_only.gist = None;
    fx::put_lexical(&store, &snapshot_only, "shared snapshot");
    let mut bare = fx::item("B", "bank", "shared");
    bare.gist = None;
    bare.snapshot = None;
    fx::put_lexical(&store, &bare, "shared bare");
    let req = request("bank", Some("shared"), 400);
    let pack = composer(&store).compose(&req, None).expect("compose");
    let memory = section(&pack, "memory");
    assert_eq!(memory.item_ids.len(), 3, "{:?}", memory.item_ids);
    assert_eq!(
        pack.form_tally,
        FormTally::fold(memory.hit_forms.iter().copied())
    );
    assert_eq!(
        pack.form_tally,
        FormTally {
            prose: 1,
            exact: 0,
            reference: 2
        }
    );
}

// Budget regression: a fixed gist fixture renders the pre-phase text and
// tokens; the only serialized delta is the additive `hit_forms` and
// `form_tally` keys (`hit_forms` after `item_ids`, `form_tally` last).
#[test]
fn gist_pack_keeps_pre_phase_text_and_tokens() {
    let store = fx::store();
    let item = fx::item("A", "bank", "shared gist");
    fx::put_lexical(&store, &item, "shared gist");
    let req = request("bank", Some("shared"), 400);
    let pack = composer(&store).compose(&req, None).expect("compose");
    let tokens = estimate_tokens("shared gist");
    let expected = format!(
        r#"{{"sections":[{{"name":"persona","tokens":0,"item_ids":[],"text":"","truncated":false}},{{"name":"memory","tokens":{tokens},"item_ids":["A"],"hit_forms":["prose"],"text":"shared gist","truncated":false}}],"total_tokens":{},"budget_tokens":400,"persona_budget_tokens":392,"memory_budget_tokens":392,"truncated_ids":[],"retrieval_ran":true,"warnings":[],"form_tally":{{"prose":1,"exact":0,"reference":0}}}}"#,
        PACK_OVERHEAD_TOKENS + tokens
    );
    assert_eq!(
        serde_json::to_string(&pack).expect("serialize pack"),
        expected
    );
}

// A hit that does not fit the budget never reaches the form record or the tally.
#[test]
fn truncated_hit_absent_from_tally_and_record() {
    let store = fx::store();
    let item = fx::item("M", "bank", "shared token content padding padding");
    fx::put_lexical(&store, &item, "shared token content padding padding");
    let req = request("bank", Some("shared"), 16);
    let pack = composer(&store).compose(&req, None).expect("compose");
    let memory = section(&pack, "memory");
    assert!(memory.item_ids.is_empty(), "{:?}", memory.item_ids);
    assert_eq!(memory.hit_forms, [] as [HitForm; 0]);
    assert!(
        pack.truncated_ids.contains(&"M".to_owned()),
        "{:?}",
        pack.truncated_ids
    );
    assert_eq!(pack.form_tally, FormTally::default());
    assert!(memory.truncated);
}

// The persona section keeps its pre-phase shape even beside a memory section with forms.
#[test]
fn persona_section_shape_unchanged() {
    let store = fx::store();
    let item = fx::item("A", "bank", "shared gist");
    fx::put_lexical(&store, &item, "shared gist");
    let mut req = request("bank", Some("shared"), 400);
    req.persona_entries = vec![entry("name", "Ada Lovelace", 0.9)];
    let pack = composer(&store).compose(&req, None).expect("compose");
    let persona = section(&pack, "persona");
    let tokens = estimate_tokens("name") + estimate_tokens("Ada Lovelace");
    let expected = format!(
        r#"{{"name":"persona","tokens":{tokens},"item_ids":["name"],"text":"name: Ada Lovelace","truncated":false}}"#
    );
    assert_eq!(
        serde_json::to_string(persona).expect("serialize persona"),
        expected
    );
    assert_eq!(persona.hit_forms, [] as [HitForm; 0]);
}

#[test]
fn debug_format_is_stable() {
    let store = fx::store();
    assert!(format!("{:?}", composer(&store)).contains("Composer"));
}

// Stored source `context` never joins the model-facing pack by default and
// never expands the total budget: context-carrying hits compose like bare ones.
#[test]
fn source_context_is_never_injected_and_never_expands_the_budget() {
    let store = fx::store();
    let mut item = fx::item("C", "bank", "shared budget probe probe probe");
    item.context = Some("top secret rollout room".to_owned());
    fx::put_lexical(&store, &item, "shared budget probe probe probe");

    let mut req = request("bank", Some("shared"), 64);
    req.persona_entries = vec![entry("name", "Ada Lovelace", 0.9)];
    let pack = composer(&store).compose(&req, None).expect("compose");
    let body = serde_json::to_string(&pack).expect("serialize pack");
    assert!(
        !body.contains("top secret rollout room"),
        "compose must not inject stored context: {body}"
    );
    assert!(
        pack.total_tokens <= 64,
        "budget 64 vs {}",
        pack.total_tokens
    );
    // The hit itself still exposes the context as retrieval metadata.
    let hits = fx::retriever(&store)
        .retrieve(&crate::types::RetrieveRequest::new("bank", "shared"))
        .expect("retrieve")
        .hits;
    let hit = hits.iter().find(|h| h.hit.item_id == "C").expect("hit");
    assert_eq!(hit.hit.context.as_deref(), Some("top secret rollout room"));
}

#[test]
fn total_tokens_never_exceeds_budget_across_sizes() {
    let store = fx::store();
    for i in 0..20 {
        let item = fx::item(
            &format!("M{i}"),
            "bank",
            "shared token padding padding padding",
        );
        fx::put_lexical(&store, &item, "shared token padding padding padding");
    }
    for budget in [9_usize, 16, 32, 64, 128] {
        let mut req = request("bank", Some("shared"), budget);
        req.persona_entries = vec![entry("name", "Ada Lovelace", 0.9)];
        let pack = composer(&store).compose(&req, None).expect("compose");
        assert!(
            pack.total_tokens <= budget,
            "budget {budget} total {}",
            pack.total_tokens
        );
    }
}
// Reference-path baseline: each marked reference costs 4 tokens (1 for the
// bare id plus the marker's 3), and that cost is the truncation margin.
#[test]
fn reference_pack_pins_total_tokens_and_truncation_boundary() {
    let store = fx::store();
    for (id, doc) in [("R", "shared bare"), ("Q", "shared pair")] {
        let mut bare = fx::item(id, "bank", "shared");
        bare.gist = None;
        bare.snapshot = None;
        fx::put_lexical(&store, &bare, doc);
    }
    let reference = estimate_tokens(&format!("{REFERENCE_MARKER}R"));
    assert_eq!(reference, 4, "13 chars: ceil(13/4)");
    let exact = PACK_OVERHEAD_TOKENS + 2 * reference;
    let tail = PACK_OVERHEAD_TOKENS + reference;
    for (budget, total, truncated) in [
        (400, exact, vec![]),
        (exact + 1, exact, vec![]),
        (exact, exact, vec![]),
        (exact - 1, tail, vec!["Q".to_owned()]),
        (exact - 3, tail, vec!["Q".to_owned()]),
    ] {
        let req = request("bank", Some("shared"), budget);
        let pack = composer(&store).compose(&req, None).expect("compose");
        assert_eq!(pack.total_tokens, total, "budget {budget}");
        assert_eq!(pack.truncated_ids, truncated, "budget {budget}");
    }
}

// An all-truncated memory section still serializes but skips its empty form
// record: the names tell the sections apart, and the pack keeps a zero tally.
#[test]
fn all_truncated_memory_section_omits_empty_hit_forms_and_keeps_zero_tally() {
    let store = fx::store();
    let item = fx::item("M", "bank", "shared token content padding padding");
    fx::put_lexical(&store, &item, "shared token content padding padding");
    let req = request("bank", Some("shared"), 16);
    let pack = composer(&store).compose(&req, None).expect("compose");
    let body = serde_json::to_string(&pack).expect("serialize pack");
    assert!(!body.contains("hit_forms"), "{body}");
    assert_eq!(
        serde_json::to_string(section(&pack, "memory")).expect("serialize memory"),
        r#"{"name":"memory","tokens":0,"item_ids":[],"text":"","truncated":true}"#
    );
    assert_eq!(
        serde_json::to_string(section(&pack, "persona")).expect("serialize persona"),
        r#"{"name":"persona","tokens":0,"item_ids":[],"text":"","truncated":false}"#
    );
    assert_eq!(
        serde_json::to_value(&pack).expect("pack json")["form_tally"],
        serde_json::json!({"prose": 0, "exact": 0, "reference": 0})
    );
}

#[test]
fn compose_request_defaults_keep_the_curve_off() {
    let req = request("bank", None, 100);
    assert!(!req.retention_curve, "curve is off by default");
    assert_eq!(req.retention_floor, 0.25, "floor default is 0.25");
}
