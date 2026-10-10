// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the bi-temporal triple type helpers.
//!
//! ## Owns
//! - Interval-algebra, visibility, validation, and wire-parse test cases
//!
//! ## Does not own
//! - Persistence, query execution, or admission gating behavior
//!
//! ## Boundary
//!
//! Changes to triple interval semantics should normally update tests here.
//! Changes to SQL drivers or store wiring should not require changes here.

use crate::{
    CONTEXT_MAX_BYTES, EpistemicKind, ErrorCode, SourceType, TimeAxis, TripleQuery, TripleRecord,
    UpdateRule, ensure_discrete_update_rule, interval_contains,
};

fn sample() -> TripleRecord {
    TripleRecord {
        id: "e1".into(),
        bank_id: "b".into(),
        item_id: "i1".into(),
        subject: "Alice".into(),
        predicate: "lives_in".into(),
        object: "Boston".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-02".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: "2024-01-02".into(),
        updated_at: None,
    }
}

#[test]
fn interval_half_open() {
    assert!(interval_contains("a", None, "a"));
    assert!(interval_contains("a", Some("c"), "b"));
    assert!(!interval_contains("a", Some("b"), "b"));
    assert!(!interval_contains("b", None, "a"));
}

#[test]
fn time_axis_and_update_rule_parse() {
    assert_eq!(TimeAxis::parse("valid").unwrap(), TimeAxis::Valid);
    assert_eq!(
        TimeAxis::parse("transaction").unwrap(),
        TimeAxis::Transaction
    );
    assert!(TimeAxis::parse("both").is_err());
    assert_eq!(UpdateRule::parse("discrete").unwrap(), UpdateRule::Discrete);
    assert_eq!(
        UpdateRule::parse("continuous").unwrap(),
        UpdateRule::Continuous
    );
    assert_eq!(TimeAxis::Valid.as_str(), "valid");
    assert_eq!(UpdateRule::Continuous.as_str(), "continuous");
}

#[test]
fn discrete_path_rejects_continuous() {
    ensure_discrete_update_rule(None).unwrap();
    ensure_discrete_update_rule(Some(UpdateRule::Discrete)).unwrap();
    let err = ensure_discrete_update_rule(Some(UpdateRule::Continuous)).unwrap_err();
    assert_eq!(err.code, ErrorCode::WrongUpdateRule);
}

#[test]
fn triple_validate_and_visibility() {
    let mut t = sample();
    t.validate().unwrap();
    assert!(t.is_open());
    assert!(t.visible_at(TimeAxis::Valid, "2024-06-01"));
    t.valid_until = Some("2024-06-01".into());
    assert!(!t.visible_at(TimeAxis::Valid, "2024-06-01"));
    t.epistemic_kind = EpistemicKind::Belief;
    assert!(t.validate().is_err());
    t.source_type = Some(SourceType::UserStated);
    t.validate().unwrap();
    t.epistemic_kind = EpistemicKind::Fact;
    assert!(t.validate().is_err());
    t.source_type = None;
    t.valid_until = Some("2023-01-01".into());
    assert!(t.validate().is_err());
}

#[test]
fn triple_validate_rejects_empty_and_ranges() {
    let mut t = sample();
    t.id.clear();
    assert!(t.validate().is_err());
    t = sample();
    t.confidence = Some(1.5);
    assert_eq!(t.validate().unwrap_err().code, ErrorCode::OutOfRange);
    t = sample();
    t.tx_until = Some("2023-01-01".into());
    assert!(t.validate().is_err());
    assert_eq!(TimeAxis::default(), TimeAxis::Valid);
    assert_eq!(UpdateRule::Discrete.as_str(), "discrete");
    assert!(UpdateRule::parse("nope").is_err());
    let q = TripleQuery {
        bank_id: "b".into(),
        subject: None,
        predicate: None,
        object: None,
        as_of: None,
        time_axis: TimeAxis::Transaction,
    };
    q.validate().unwrap();
    assert!(!sample().visible_at(TimeAxis::Transaction, "2023-01-01"));
}

#[test]
fn triple_query_requires_bank() {
    let q = TripleQuery {
        bank_id: String::new(),
        subject: None,
        predicate: None,
        object: None,
        as_of: None,
        time_axis: TimeAxis::Valid,
    };
    assert_eq!(q.validate().unwrap_err().code, ErrorCode::MissingBank);
}

#[test]
fn triple_context_serialization_omits_absent_key() {
    // A record written without context must not carry an empty-string
    // sentinel: the key is omitted from the wire form entirely.
    let without = serde_json::to_string(&sample()).unwrap();
    assert!(!without.contains("context"), "{without}");
    let mut with = sample();
    with.context = Some("quarterly review minutes".into());
    let with_json = serde_json::to_string(&with).unwrap();
    assert!(
        with_json.contains("\"context\":\"quarterly review minutes\""),
        "{with_json}"
    );
    // The context field is a read projection only, but populated values still
    // obey the shared bounded-context rules when validated.
    with.validate().unwrap();
}

#[test]
fn triple_record_context_validation() {
    // Present values must be non-empty and at most
    // CONTEXT_MAX_BYTES UTF-8 bytes; `None` stays valid.
    let mut t = sample();
    t.context = Some("quarterly review minutes".into());
    t.validate().unwrap();
    t.context = Some(String::new());
    assert_eq!(t.validate().unwrap_err().code, ErrorCode::InvalidArgument);
    t.context = Some("x".repeat(CONTEXT_MAX_BYTES + 1));
    assert_eq!(t.validate().unwrap_err().code, ErrorCode::OutOfRange);
    t.context = Some("x".repeat(CONTEXT_MAX_BYTES));
    t.validate().unwrap();
    t.context = None;
    t.validate().unwrap();
}

#[test]
fn triple_context_defaults_on_deserialize() {
    let without = sample();
    let json = serde_json::to_value(&without).unwrap();
    let parsed: TripleRecord = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.context, None);
    let mut with = sample();
    with.context = Some("kept".into());
    let parsed: TripleRecord =
        serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
    assert_eq!(parsed, with);
}
