// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the retrieval response types that facts and beliefs produce.
//!
//! ## Owns
//! - Golden JSON for a fact hit, a belief item hit, and a belief-object hit, so a wire change is a visible failure
//! - The `context` key: present when the item carries context, absent when it does not
//! - The optional `exact_render` key: absent until set, round trips when set, and an old payload without it still loads
//! - `SearchDomain` item predicates, string parsing, and stable wire strings
//!
//! ## Does not own
//! - The `RetrieveHit` and `SearchDomain` definitions themselves
//! - Retrieval ranking, scoring, or orchestration
//! - The rendering text and byte cap of an `exact_render` value
//!
//! ## Boundary
//!
//! Changes to the retrieval response contract should normally require changes here.
//! Changes to retrieval orchestration should not require changes here.

use super::*;
use crate::belief::{BeliefObject, ConfidenceEntry};
use crate::item::{EpisodicType, ItemKind, SemanticCategory};

fn base_item() -> MemoryItem {
    MemoryItem {
        id: "sem-1".into(),
        bank_id: "bank".into(),
        subject_id: "sub".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.99),
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: Some("likes rust".into()),
        valid_from: "2026-01-01".into(),
        valid_until: None,
        tx_from: "2026-01-01".into(),
        tx_until: None,
        created_at: "2026-01-01".into(),
        updated_at: "2026-01-01".into(),
        context: None,
    }
}

#[test]
fn fact_hit_golden_json_has_no_trajectory() {
    let item = base_item();
    item.validate().unwrap();
    let hit = RetrieveHit::from_item(&item);
    assert_eq!(hit.epistemic_kind, EpistemicKind::Fact);
    assert!(hit.source_type.is_none());
    assert!(hit.confidence.is_none());
    assert!(!hit.requires_confidence_check);
    let json = serde_json::to_string(&hit).unwrap();
    assert_eq!(
        json,
        r#"{"item_id":"sem-1","bank_id":"bank","kind":"semantic","category":"persona","epistemic_kind":"fact","gist":"likes rust","requires_confidence_check":false}"#
    );
    let back: RetrieveHit = serde_json::from_str(&json).unwrap();
    assert_eq!(back, hit);
}

#[test]
fn belief_item_hit_golden_json_surfaces_confidence() {
    let mut item = base_item();
    item.id = "ep-1".into();
    item.kind = ItemKind::Episodic;
    item.category = None;
    item.episodic_type = Some(EpisodicType::Triple);
    item.epistemic_kind = EpistemicKind::Belief;
    item.source_type = Some(SourceType::UserStated);
    item.confidence = Some(0.9);
    item.gist = Some("s p o".into());
    item.validate().unwrap();
    let json = serde_json::to_string(&RetrieveHit::from_item(&item)).unwrap();
    assert_eq!(
        json,
        r#"{"item_id":"ep-1","bank_id":"bank","kind":"episodic","episodic_type":"triple","epistemic_kind":"belief","source_type":"user_stated","confidence":0.9,"gist":"s p o","requires_confidence_check":true}"#
    );
}

#[test]
fn belief_object_hit_uses_current_history_entry() {
    let belief = BeliefObject {
        context: None,
        context_as_of: None,
        id: "bel-1".into(),
        bank_id: "bank".into(),
        subject_id: "sub".into(),
        proposition: "user likes coffee".into(),
        confidence_history: vec![
            ConfidenceEntry::new("2026-06-01", 0.4, "turn_1", SourceType::AgentInferred).unwrap(),
            ConfidenceEntry::new("2026-07-14", 0.65, "turn_2", SourceType::AgentInferred).unwrap(),
        ],
        created_at: "2026-06-01".into(),
        updated_at: "2026-07-14".into(),
    };
    let hit = RetrieveHit::from_belief(&belief);
    assert_eq!(hit.kind, "belief");
    assert_eq!(hit.source_type, Some(SourceType::AgentInferred));
    assert_eq!(hit.confidence, Some(0.65));
    assert!(hit.requires_confidence_check);
    let json = serde_json::to_string(&hit).unwrap();
    assert_eq!(
        json,
        r#"{"item_id":"bel-1","bank_id":"bank","kind":"belief","epistemic_kind":"belief","source_type":"agent_inferred","confidence":0.65,"gist":"user likes coffee","requires_confidence_check":true}"#
    );
}

#[test]
fn item_hit_carries_context_and_omits_key_when_absent() {
    // An item stored WITH context surfaces it on the hit and in its JSON.
    let mut with_context = base_item();
    with_context.context = Some("from a team chat".into());
    with_context.validate().unwrap();
    let hit = RetrieveHit::from_item(&with_context);
    assert_eq!(hit.context.as_deref(), Some("from a team chat"));
    let json = serde_json::to_string(&hit).unwrap();
    assert!(json.contains(r#""context":"from a team chat""#), "{json}");
    let back: RetrieveHit = serde_json::from_str(&json).unwrap();
    assert_eq!(back, hit);

    // An item WITHOUT context omits the `context` key entirely.
    let bare = base_item();
    bare.validate().unwrap();
    let hit = RetrieveHit::from_item(&bare);
    assert!(hit.context.is_none());
    let json = serde_json::to_string(&hit).unwrap();
    assert!(!json.contains("\"context\""), "{json}");
}

#[test]
fn exact_render_is_absent_until_somebody_sets_it() {
    // `from_item` and `from_belief` never produce a rendering, so the field is
    // absent from the wire rather than present-and-empty.
    let mut with_snapshot = base_item();
    with_snapshot.snapshot = Some(serde_json::json!({"unit_id": "u", "hub_id": "h"}));
    with_snapshot.validate().unwrap();
    let hit = RetrieveHit::from_item(&with_snapshot);
    assert!(
        hit.exact_render.is_none(),
        "a snapshot on the item does not by itself produce a rendering: {:?}",
        hit.exact_render
    );
    let json = serde_json::to_string(&hit).expect("serialize hit");
    assert!(!json.contains("exact_render"), "{json}");
    assert!(!json.contains("\"exact_render\":null"), "{json}");
}

#[test]
fn exact_render_round_trips_when_present() {
    let mut hit = RetrieveHit::from_item(&base_item());
    hit.exact_render = Some(r#"{"hub_id":"h","unit_id":"u"}"#.to_owned());
    let json = serde_json::to_string(&hit).expect("serialize hit");
    assert!(
        json.contains(r#""exact_render":"{\"hub_id\":\"h\",\"unit_id\":\"u\"}""#),
        "{json}"
    );
    let back: RetrieveHit = serde_json::from_str(&json).expect("deserialize hit");
    assert_eq!(back.exact_render, hit.exact_render);
    assert_eq!(back, hit, "the round trip changes nothing");
}

#[test]
fn a_hit_json_without_the_exact_render_key_deserializes() {
    // The `#[serde(default)]` half of the pairing: a payload written before the
    // field existed has no `exact_render` key and must still load.
    let json = r#"{"item_id":"sem-1","bank_id":"bank","kind":"semantic","epistemic_kind":"fact","requires_confidence_check":false}"#;
    let back: RetrieveHit = serde_json::from_str(json).expect("a payload without the key loads");
    assert!(
        back.exact_render.is_none(),
        "an absent key defaults to None: {:?}",
        back.exact_render
    );
    let reserialized = serde_json::to_string(&back).expect("serialize");
    assert_eq!(
        reserialized, json,
        "the round trip adds no key the payload did not have"
    );
}

#[test]
fn search_domain_predicates_and_parse() {
    assert_eq!(
        SearchDomain::Task.item_predicates(),
        ("episodic", None, Some("task"))
    );
    assert_eq!(
        SearchDomain::Persona.item_predicates(),
        ("semantic", Some("persona"), None)
    );
    assert_eq!(
        SearchDomain::Semantic.item_predicates(),
        ("semantic", None, None)
    );
    assert_eq!(
        SearchDomain::parse_all(&["task".into(), "persona".into()]).unwrap(),
        vec![SearchDomain::Task, SearchDomain::Persona]
    );
    assert!(SearchDomain::parse("images").is_err());
    assert!(SearchDomain::parse_all(&["bad".into()]).is_err());
    // Wire strings are stable for every domain.
    assert_eq!(SearchDomain::Semantic.as_str(), "semantic");
    assert_eq!(SearchDomain::Episodic.as_str(), "episodic");
    assert_eq!(SearchDomain::Task.as_str(), "task");
    assert_eq!(SearchDomain::Failure.as_str(), "failure");
    assert_eq!(SearchDomain::Temporal.as_str(), "temporal");
    assert_eq!(SearchDomain::Persona.as_str(), "persona");
}
