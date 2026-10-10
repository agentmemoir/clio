// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Unit tests for belief shapes and identity normalization.

use super::*;
use crate::item::{EpistemicKind, ItemKind, MemoryItem};

fn entry(as_of: &str, confidence: f64, source_type: SourceType) -> ConfidenceEntry {
    ConfidenceEntry::new(as_of, confidence, "turn_1", source_type).expect("entry")
}

#[test]
fn proposition_key_normalizes_case_and_whitespace() {
    assert_eq!(
        proposition_key("  User   Likes COFFEE "),
        "user likes coffee"
    );
    assert_eq!(proposition_key("user likes coffee"), "user likes coffee");
    assert_eq!(proposition_key("User\tLikes\nCoffee"), "user likes coffee");
    assert_eq!(proposition_key("   "), "");
}

#[test]
fn confidence_entry_validation() {
    assert!(ConfidenceEntry::new("t", 0.0, "e", SourceType::UserStated).is_ok());
    assert!(ConfidenceEntry::new("t", 1.0, "e", SourceType::UserStated).is_ok());
    assert_eq!(
        ConfidenceEntry::new("t", -0.1, "e", SourceType::UserStated)
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        ConfidenceEntry::new("t", 1.1, "e", SourceType::UserStated)
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        ConfidenceEntry::new("t", f64::NAN, "e", SourceType::UserStated)
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        ConfidenceEntry::new("", 0.5, "e", SourceType::UserStated)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        ConfidenceEntry::new("t", 0.5, "", SourceType::UserStated)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert!(check_confidence(f64::INFINITY).is_err());
}

fn belief(history: Vec<ConfidenceEntry>) -> BeliefObject {
    BeliefObject {
        context: None,
        context_as_of: None,
        id: "b1".into(),
        bank_id: "bank".into(),
        subject_id: "sub".into(),
        proposition: "user likes coffee".into(),
        confidence_history: history,
        created_at: "2026-06-01".into(),
        updated_at: "2026-06-01".into(),
    }
}

#[test]
fn current_confidence_and_source_type_track_last_entry() {
    let mut b = belief(vec![]);
    assert!(b.current_entry().is_none());
    assert!(b.current_confidence().is_none());
    assert!(b.current_source_type().is_none());
    b.append(entry("2026-06-01", 0.4, SourceType::AgentInferred))
        .unwrap();
    b.append(entry("2026-07-14", 0.65, SourceType::AgentInferred))
        .unwrap();
    b.append(entry("2026-09-02", 0.9, SourceType::UserStated))
        .unwrap();
    assert_eq!(b.confidence_history.len(), 3);
    assert_eq!(b.current_confidence(), Some(0.9));
    assert_eq!(b.current_source_type(), Some(SourceType::UserStated));
    assert_eq!(b.updated_at, "2026-09-02");
}

#[test]
fn as_of_confidence_picks_latest_at_or_before() {
    let b = belief(vec![
        entry("2026-06-01", 0.4, SourceType::AgentInferred),
        entry("2026-07-14", 0.65, SourceType::AgentInferred),
        entry("2026-09-02", 0.9, SourceType::UserStated),
    ]);
    assert_eq!(b.as_of_confidence("2026-05-01"), None);
    assert_eq!(b.as_of_confidence("2026-06-01"), Some(0.4));
    assert_eq!(b.as_of_confidence("2026-07-14"), Some(0.65));
    assert_eq!(b.as_of_confidence("2026-08-01"), Some(0.65));
    assert_eq!(b.as_of_confidence("2027-01-01"), Some(0.9));
}

#[test]
fn belief_validation_rejects_bad_shapes() {
    let mut b = belief(vec![]);
    b.validate().unwrap();
    b.id = String::new();
    assert!(b.validate().is_err());
    let mut b = belief(vec![]);
    b.proposition = String::new();
    assert!(b.validate().is_err());
    let mut b = belief(vec![]);
    b.confidence_history.push(ConfidenceEntry {
        as_of: "2026-06-01".into(),
        confidence: 2.0,
        evidence_ref: "e".into(),
        source_type: SourceType::ThirdParty,
    });
    assert!(b.validate().is_err());
    let mut b = belief(vec![]);
    assert!(
        b.append(ConfidenceEntry {
            as_of: "t".into(),
            confidence: 0.5,
            evidence_ref: String::new(),
            source_type: SourceType::UserStated,
        })
        .is_err()
    );
}

#[test]
fn fact_items_never_carry_a_trajectory() {
    let item = MemoryItem {
        id: "i1".into(),
        bank_id: "bank".into(),
        subject_id: "sub".into(),
        kind: ItemKind::Semantic,
        category: Some(crate::SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.99),
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: Some("g".into()),
        valid_from: "t".into(),
        valid_until: None,
        tx_from: "t".into(),
        tx_until: None,
        created_at: "t".into(),
        updated_at: "t".into(),
        context: None,
    };
    item.validate().unwrap();
}

#[test]
fn legacy_wire_payload_without_context_as_of_deserializes_none() {
    // The store derives `context_as_of`; a payload written before the stamp
    // existed (or a caller omitting it) must deserialize with `None`, not fail.
    let legacy = serde_json::json!({
        "id": "b1",
        "bank_id": "bank",
        "subject_id": "sub",
        "proposition": "user likes coffee",
        "confidence_history": [{
            "as_of": "2026-06-01",
            "confidence": 0.4,
            "evidence_ref": "turn_1",
            "source_type": "agent_inferred"
        }],
        "created_at": "2026-06-01",
        "updated_at": "2026-06-01"
    });
    let parsed: BeliefObject = serde_json::from_value(legacy).expect("legacy payload");
    assert_eq!(parsed.context_as_of, None);
    // The stamp is never serialized when absent, and round-trips when present.
    let mut stamped = belief(vec![entry("2026-06-01", 0.4, SourceType::UserStated)]);
    stamped.context = Some("ctx".into());
    stamped.context_as_of = Some("2026-06-01T00:00:00Z".into());
    let body = serde_json::to_string(&stamped).expect("serialize");
    assert!(body.contains("\"context_as_of\""), "{body}");
    let back: BeliefObject = serde_json::from_str(&body).expect("round-trip");
    assert_eq!(back.context_as_of.as_deref(), Some("2026-06-01T00:00:00Z"));
}
