// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Unit tests for memory-item domain validation.

use crate::{
    EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory, SourceType,
};
use serde_json::json;

fn base_semantic() -> MemoryItem {
    MemoryItem {
        id: "i1".into(),
        bank_id: "b".into(),
        subject_id: "s".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::TaskSpec),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.0),
        source_ref: Some("turn_1".into()),
        context: None,
        snapshot: Some(json!({"port": 8080})),
        gist: Some("listening on 8080".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
    }
}

#[test]
fn parses_and_rejects_enums() {
    assert_eq!(ItemKind::parse("semantic").unwrap(), ItemKind::Semantic);
    assert!(ItemKind::parse("x").is_err());
    assert_eq!(
        SemanticCategory::parse("persona").unwrap(),
        SemanticCategory::Persona
    );
    assert!(SemanticCategory::parse("belief").is_err());
    assert_eq!(EpisodicType::parse("task").unwrap(), EpisodicType::Task);
    assert!(EpisodicType::parse("other").is_err());
    assert!(EpisodicType::parse("belief").is_err());
    assert_eq!(EpistemicKind::parse("fact").unwrap(), EpistemicKind::Fact);
    assert!(EpistemicKind::parse("opinion").is_err());
    assert_eq!(
        SourceType::parse("user_stated").unwrap(),
        SourceType::UserStated
    );
    assert!(SourceType::parse("x").is_err());
    assert_eq!(ItemKind::Episodic.as_str(), "episodic");
    assert_eq!(SemanticCategory::Schema.as_str(), "schema");
    assert_eq!(EpisodicType::Gist.as_str(), "gist");
    assert_eq!(EpistemicKind::Belief.as_str(), "belief");
    assert_eq!(SourceType::ThirdParty.as_str(), "third_party");
    assert_eq!(SourceType::AgentInferred.as_str(), "agent_inferred");
    assert_eq!(
        SemanticCategory::OutputConstraint.as_str(),
        "output_constraint"
    );
    assert_eq!(SemanticCategory::ToolConfig.as_str(), "tool_config");
    assert_eq!(SemanticCategory::TaskSpec.as_str(), "task_spec");
    assert_eq!(SemanticCategory::Persona.as_str(), "persona");
    assert_eq!(EpisodicType::Triple.as_str(), "triple");
    assert_eq!(EpisodicType::Failure.as_str(), "failure");
    assert_eq!(EpisodicType::Temporal.as_str(), "temporal");
}

#[test]
fn semantic_category_help_list_matches_the_whitelist() {
    // `ALL`/`WIRE` are generated from one list, so every entry must round-trip
    // and the wire strings must stay in the documented order.
    assert_eq!(SemanticCategory::ALL.len(), 5);
    for &(category, wire, description) in SemanticCategory::ALL {
        assert!(!description.is_empty(), "{category:?} needs a description");
        assert_eq!(category.as_str(), wire);
        assert_eq!(SemanticCategory::parse(wire).unwrap(), category);
    }
    let expected = [
        "task_spec",
        "schema",
        "tool_config",
        "output_constraint",
        "persona",
    ];
    assert_eq!(SemanticCategory::WIRE, expected.as_slice());
}

#[test]
fn fr14_and_kind_rules() {
    let mut item = base_semantic();
    item.validate().unwrap();
    item.epistemic_kind = EpistemicKind::Belief;
    assert!(item.validate().is_err());
    item.source_type = Some(SourceType::AgentInferred);
    item.validate().unwrap();
    item.epistemic_kind = EpistemicKind::Fact;
    assert!(item.validate().is_err());
    item.source_type = None;
    item.kind = ItemKind::Episodic;
    item.category = Some(SemanticCategory::Persona);
    item.episodic_type = Some(EpisodicType::Failure);
    assert!(item.validate().is_err());
    item.category = None;
    item.validate().unwrap();
    item.episodic_type = None;
    assert!(item.validate().is_err());
    item.kind = ItemKind::Semantic;
    item.category = None;
    assert!(item.validate().is_err());
    item.category = Some(SemanticCategory::Schema);
    item.episodic_type = Some(EpisodicType::Gist);
    assert!(item.validate().is_err());
    item.episodic_type = None;
    item.id.clear();
    assert!(item.validate().is_err());
    item.id = "i1".into();
    item.valid_from.clear();
    assert!(item.validate().is_err());
    item.valid_from = "t0".into();
    item.confidence = Some(1.5);
    assert_eq!(item.validate().unwrap_err().code, ErrorCode::OutOfRange);
    item.confidence = Some(0.5);
    item.tx_from.clear();
    assert!(item.validate().is_err());
    item.tx_from = "t0".into();
    item.bank_id.clear();
    assert!(item.validate().is_err());
    item.bank_id = "b".into();
    item.subject_id.clear();
    assert!(item.validate().is_err());
    item.subject_id = "s".into();
    item.admission_score = Some(-0.1);
    assert_eq!(item.validate().unwrap_err().code, ErrorCode::OutOfRange);
    item.admission_score = None;
    item.confidence = None;
    item.validate().unwrap();
}

#[test]
fn context_validation_rejects_empty_and_oversized_only() {
    assert_eq!(crate::CONTEXT_MAX_BYTES, 4096);
    crate::validate_context("team chat").unwrap();
    crate::validate_context(&"a".repeat(crate::CONTEXT_MAX_BYTES)).unwrap();
    assert_eq!(
        crate::validate_context("").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let oversized = crate::validate_context(&"a".repeat(crate::CONTEXT_MAX_BYTES + 1))
        .expect_err("oversized context");
    assert_eq!(oversized.code, ErrorCode::OutOfRange);
    // The limit is UTF-8 bytes, not characters: a multi-byte value can exceed
    // the published maximum while staying under 4096 code points.
    let multibyte = "é".repeat(2049);
    assert_eq!(multibyte.chars().count(), 2049);
    assert_eq!(
        crate::validate_context(&multibyte).unwrap_err().code,
        ErrorCode::OutOfRange
    );
}

#[test]
fn item_validate_applies_context_rules() {
    let mut item = base_semantic();
    item.context = Some(String::new());
    assert_eq!(
        item.validate().unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    item.context = Some("a".repeat(crate::CONTEXT_MAX_BYTES + 1));
    assert_eq!(item.validate().unwrap_err().code, ErrorCode::OutOfRange);
    item.context = Some("team chat".into());
    item.validate().unwrap();
}

#[test]
fn context_field_round_trips_and_old_json_stays_readable() {
    let mut item = base_semantic();
    item.context = Some("support ticket".into());
    let json = serde_json::to_value(&item).expect("serialize");
    assert_eq!(json["context"], "support ticket");
    let parsed: MemoryItem = serde_json::from_value(json).expect("deserialize");
    assert_eq!(parsed, item);

    // A payload written before the field existed must deserialize as absent.
    let legacy = serde_json::json!({
        "id": "i1", "bank_id": "b", "subject_id": "s", "kind": "semantic",
        "category": "task_spec", "episodic_type": null, "epistemic_kind": "fact",
        "source_type": null, "confidence": 0.9, "admission_score": 0.0,
        "source_ref": "turn_1", "snapshot": {"port": 8080}, "gist": "g",
        "valid_from": "t0", "valid_until": null, "tx_from": "t0", "tx_until": null,
        "created_at": "t0", "updated_at": "t0"
    });
    let parsed: MemoryItem = serde_json::from_value(legacy).expect("legacy deserialize");
    assert!(parsed.context.is_none());
    // Absent context stays out of serialized output (no empty-string sentinel).
    let out = serde_json::to_value(&parsed).expect("serialize");
    assert!(out.get("context").is_none());
}
