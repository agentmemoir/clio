// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of persona-document shapes.
//!
//! ## Owns
//! - Validation rules for stable entries, preferences, and documents
//! - Vocab contract: provenance field is `source`, never `source_type`
//!
//! ## Does not own
//! - Persistence or tool-handler tests
//!
//! ## Boundary
//!
//! Changes to persona validation should normally update tests here.

use crate::CONTEXT_MAX_BYTES;
use crate::persona::{
    PersonaDocument, PersonaPreference, PersonaStableEntry, validate_persona_source,
};

fn stable(score: f64) -> PersonaStableEntry {
    PersonaStableEntry {
        context: None,
        bank_id: "b".into(),
        key: "communication_style".into(),
        value: "concise".into(),
        confidence: Some(0.8),
        source: "inferred".into(),
        source_ref: Some("turn-1".into()),
        admission_score: score,
        as_of: "2026-09-10".into(),
        valid_until: None,
    }
}

fn pref(score: f64) -> PersonaPreference {
    PersonaPreference {
        context: None,
        bank_id: "b".into(),
        key: "brevity_affinity".into(),
        value: 0.82,
        trend: 0.72,
        confidence: Some(0.8),
        source: "inferred".into(),
        source_ref: Some("turn-1".into()),
        admission_score: score,
        as_of: "2026-09-10".into(),
    }
}

#[test]
fn valid_entries_pass() {
    stable(0.9).validate().unwrap();
    pref(0.7).validate().unwrap();
    let doc = PersonaDocument {
        stable: vec![stable(0.9)],
        preferences: vec![pref(0.7)],
    };
    doc.validate().unwrap();
}

#[test]
fn scores_and_confidences_are_unit_bounded() {
    for bad in [-0.1_f64, 1.1, f64::NAN] {
        assert!(stable(bad).validate().is_err(), "score {bad}");
        assert!(pref(bad).validate().is_err(), "score {bad}");
    }
    let mut s = stable(0.9);
    s.confidence = Some(1.5);
    assert!(s.validate().is_err());
    let mut p = pref(0.5);
    p.value = f64::NAN;
    assert!(p.validate().is_err());
    // NaN/Inf trend is rejected alongside NaN/Inf value (numeric domain).
    let mut p = pref(0.5);
    p.trend = f64::NAN;
    assert!(p.validate().is_err());
    let mut p = pref(0.5);
    p.trend = f64::INFINITY;
    assert!(p.validate().is_err());
}

#[test]
fn required_fields_are_enforced() {
    let mut s = stable(0.9);
    s.key = String::new();
    assert!(s.validate().is_err());
    s.key = "k".into();
    s.value = String::new();
    assert!(s.validate().is_err());
    s.value = "v".into();
    s.source = "stated".into();
    assert!(s.validate().is_err(), "only explicit|inferred allowed");
    s.source = "explicit".into();
    s.as_of = String::new();
    assert!(s.validate().is_err());
    let mut p = pref(0.5);
    p.key = String::new();
    assert!(p.validate().is_err());
    p.key = "k".into();
    p.source = "user_stated".into();
    assert!(p.validate().is_err());
    assert_eq!(
        validate_persona_source("third_party").unwrap_err().code,
        crate::ErrorCode::InvalidArgument
    );
    // A superseded (closed) stable version validates too; it just stays hidden
    // from document assembly.
    let mut closed = stable(0.5);
    closed.valid_until = Some("2026-09-11".into());
    closed.validate().unwrap();
}

/// Persona `context` is optional, bounded, and never empty.
#[test]
fn context_validation() {
    let mut s = stable(0.9);
    s.context = Some("why this entry exists".into());
    s.validate().unwrap();
    s.context = Some(String::new());
    assert_eq!(
        s.validate().unwrap_err().code,
        crate::ErrorCode::InvalidArgument
    );
    s.context = Some("x".repeat(CONTEXT_MAX_BYTES + 1));
    assert_eq!(s.validate().unwrap_err().code, crate::ErrorCode::OutOfRange);
    s.context = Some("x".repeat(CONTEXT_MAX_BYTES));
    s.validate().unwrap();
    s.context = None;
    s.validate().unwrap();

    let mut p = pref(0.5);
    p.context = Some("why this preference exists".into());
    p.validate().unwrap();
    p.context = Some(String::new());
    assert_eq!(
        p.validate().unwrap_err().code,
        crate::ErrorCode::InvalidArgument
    );
    p.context = Some("x".repeat(CONTEXT_MAX_BYTES + 1));
    assert_eq!(p.validate().unwrap_err().code, crate::ErrorCode::OutOfRange);
    p.context = Some("x".repeat(CONTEXT_MAX_BYTES));
    p.validate().unwrap();
    p.context = None;
    p.validate().unwrap();
}

/// provenance serializes as `source`; `source_type` never appears.
#[test]
fn vocab_contract_source_not_source_type() {
    let json = serde_json::to_string(&pref(0.5)).unwrap();
    assert!(json.contains("\"source\":\"inferred\""), "{json}");
    assert!(!json.contains("source_type"), "{json}");
    let stable_json = serde_json::to_string(&stable(0.5)).unwrap();
    assert!(stable_json.contains("\"source\":\"inferred\""));
    assert!(!stable_json.contains("source_type"));
}
