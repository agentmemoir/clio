// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use crate::consolidate::{
    VerifiedCandidate, consolidate, discrete_key, fingerprint, normalize_surface,
};
use crate::extract::ExtractCandidate;
use serde_json::json;

fn vc(chunk: &str, snap: serde_json::Value, text: &str) -> VerifiedCandidate {
    VerifiedCandidate {
        chunk_id: chunk.into(),
        source_text: text.into(),
        candidate: ExtractCandidate {
            snapshot: Some(snap),
            gist: Some("g".into()),
            source_ref: chunk.into(),
            diagnostic: None,
        },
        confidence: 0.8,
    }
}

#[test]
fn normalize_collapses_case_and_space() {
    assert_eq!(normalize_surface("  Foo   BAR "), "foo bar");
}

#[test]
fn t03b_identical_fingerprint_merges() {
    let a = vc("c0", json!({"entity": "Ada"}), "Ada");
    let b = vc("c1", json!({"entity": " ada "}), "Ada again");
    assert_eq!(fingerprint(&a.candidate), fingerprint(&b.candidate));
    let out = consolidate(&[a, b]);
    assert_eq!(out.units.len(), 1);
    assert_eq!(out.stats.merges, 1);
    assert_eq!(out.units[0].chunk_ids, vec!["c0", "c1"]);
    assert!((out.units[0].confidence - 0.8).abs() < f64::EPSILON);
}

#[test]
fn t03_boston_vs_nyc_no_merge() {
    let a = vc(
        "c0",
        json!({"subject": "Alice", "predicate": "lives_in", "object": "Boston"}),
        "Alice lives in Boston",
    );
    let b = vc(
        "c1",
        json!({"subject": "Alice", "predicate": "lives_in", "object": "NYC"}),
        "Alice lives in NYC",
    );
    assert_eq!(
        discrete_key(a.candidate.snapshot.as_ref().unwrap()).as_deref(),
        Some("alice|lives_in")
    );
    let out = consolidate(&[a, b]);
    assert_eq!(out.units.len(), 2);
    assert_eq!(out.stats.contradiction_splits, 1);
    assert!(out.units.iter().all(|u| {
        u.snapshot
            .as_ref()
            .and_then(|s| s.get("object"))
            .and_then(|o| o.as_str())
            .is_some_and(|o| o == "Boston" || o == "NYC")
    }));
    assert!(out.units.iter().any(|u| {
        u.triple_handoff_note
            .as_ref()
            .is_some_and(|n| n.contains("triple"))
    }));
}

#[test]
fn attribute_key_contradiction_splits() {
    let a = vc(
        "c0",
        json!({"attribute": "city", "value": "Boston", "entity": "Ada"}),
        "Boston",
    );
    let b = vc(
        "c1",
        json!({"attribute": "city", "value": "NYC", "entity": "Ada"}),
        "NYC",
    );
    assert_eq!(
        discrete_key(a.candidate.snapshot.as_ref().unwrap()).as_deref(),
        Some("attr:city")
    );
    let out = consolidate(&[a, b]);
    assert_eq!(out.units.len(), 2);
}

#[test]
fn confidence_out_of_range_defaults_to_one() {
    let mut a = vc("c0", json!({"entity": "Ada"}), "Ada");
    a.confidence = 2.0;
    let out = consolidate(&[a]);
    assert!((out.units[0].confidence - 1.0).abs() < f64::EPSILON);
}
