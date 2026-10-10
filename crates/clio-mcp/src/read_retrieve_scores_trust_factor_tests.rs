// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the payload-level `trust_factor` contract
//! tests: the six-key `scores` set the key belongs to, the finiteness and
//! positivity of every reported value, and the exact default multiplier each
//! provenance class receives.
//!
//! ## Owns
//! - The exact six-key `scores` set assertion over the MCP `retrieve` payload
//! - The numbers-never-null assertion over `scores.trust_factor`
//! - The pinned default factor per provenance class (fact and user-stated
//!   beliefs `1.0`, third-party beliefs `0.95`, agent-inferred beliefs `0.85`)
//!
//! ## Does not own
//! - The trust multiplier mapping and its computation (`clio-retrieve`
//!   `hybrid_rank::trust_factor_for`)
//! - The numbers-or-null security property and the `age_factor` range
//!   (`read_scores_age_factor_tests`)
//! - The binding parity tests (`read_scores_tests`)
//!
//! ## Boundary
//!
//! Changes to what the fixtures contain should normally require changes in
//! the shared support module (`read_scores_test_support.rs`).
//! Changes to a default trust ratio should normally require changes here.
//!
//! Keep this module focused on the payload-level `trust_factor` contract.

use super::read_scores_test_support::{base_item, call, hits, put, scores_of, state};
use crate::runtime::McpState;
use clio_types::{EpistemicKind, SourceType};
use serde_json::{Value, json};

/// A belief item with its provenance source set, seeded like `base_item`.
fn put_belief(state: &McpState, id: &str, gist: &str, source: SourceType) {
    let mut item = base_item(id, "bank-a", gist);
    item.epistemic_kind = EpistemicKind::Belief;
    item.source_type = Some(source);
    put(state, &item, gist);
}

/// The bit pattern of every returned hit's `trust_factor`, in payload order.
///
/// `scores_of` also proves the exact documented key set on every hit. The bit
/// pattern is the pinned value, not a rounded comparison: the factor is
/// carried through unchanged, so the reported number must be exactly the one
/// the shaping site applied.
fn trust_factor_bits(out: &Value) -> Vec<u64> {
    hits(out)
        .iter()
        .map(|hit| {
            scores_of(hit)["trust_factor"]
                .as_f64()
                .unwrap_or_else(|| panic!("trust_factor is a number, never null: {hit}"))
                .to_bits()
        })
        .collect()
}

/// Retrieve one seeded item's hits and return their `trust_factor` bits.
fn factor_bits_for_one(state: &McpState, query: &str) -> Vec<u64> {
    let out = call(state, "retrieve", json!({"bank": "bank-a", "query": query}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let factors = trust_factor_bits(&out);
    assert!(!factors.is_empty(), "the seeded item matched: {out}");
    factors
}

/// The payload's `scores` object carries exactly the six documented keys, with
/// `trust_factor` among them.
#[test]
fn payload_scores_carry_exactly_six_keys_including_trust_factor() {
    let state = state();
    put(
        &state,
        &base_item("K", "bank-a", "trust factor key set"),
        "trust factor key set",
    );

    let out = call(
        &state,
        "retrieve",
        json!({"bank": "bank-a", "query": "trust factor key set"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(!hits(&out).is_empty(), "{out}");
    for hit in hits(&out) {
        let scores = hit["scores"].as_object().expect("scores object");
        let mut keys: Vec<&str> = scores.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "age_factor",
                "final",
                "keyword",
                "reranker",
                "semantic",
                "trust_factor"
            ],
            "{hit}"
        );
        assert!(scores.contains_key("trust_factor"), "{hit}");
    }
}

/// Every returned hit reports `trust_factor` as a finite number above `0` and
/// at most `1.0`; the key is always present and never null.
#[test]
fn trust_factor_is_always_a_finite_number_never_null() {
    let state = state();
    put(
        &state,
        &base_item("N", "bank-a", "trust factor numeric"),
        "trust factor numeric",
    );

    let out = call(
        &state,
        "retrieve",
        json!({"bank": "bank-a", "query": "trust factor numeric"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(!hits(&out).is_empty(), "{out}");
    for hit in hits(&out) {
        let factor = scores_of(hit)["trust_factor"]
            .as_f64()
            .unwrap_or_else(|| panic!("numeric trust_factor, never null: {hit}"));
        assert!(factor.is_finite(), "finite trust_factor: {hit}");
        assert!(
            (0.0..=1.0).contains(&factor),
            "trust factor within [0, 1]: {hit}"
        );
        assert!(factor > 0.0, "trust factor above zero: {hit}");
    }
}

/// A fact is the reference class: the default multiplier leaves it at `1.0`.
#[test]
fn fact_hits_report_trust_factor_one() {
    let state = state();
    put(
        &state,
        &base_item("F", "bank-a", "fact trust factor"),
        "fact trust factor",
    );

    for factor in factor_bits_for_one(&state, "fact trust factor") {
        assert_eq!(factor, 1.0f64.to_bits(), "fact factor pinned to 1.0");
    }
}

/// A user-stated belief keeps the default `1.0` ratio: the source is trusted
/// like a fact.
#[test]
fn user_stated_beliefs_report_trust_factor_one() {
    let state = state();
    put_belief(
        &state,
        "U",
        "trust factor user stated belief",
        SourceType::UserStated,
    );

    for factor in factor_bits_for_one(&state, "trust factor user stated belief") {
        assert_eq!(factor, 1.0f64.to_bits(), "user-stated factor pinned to 1.0");
    }
}

/// A third-party belief is demoted by the shipping `0.95` multiplier.
#[test]
fn third_party_beliefs_report_trust_factor_0_95() {
    let state = state();
    put_belief(
        &state,
        "T",
        "trust factor third party belief",
        SourceType::ThirdParty,
    );

    for factor in factor_bits_for_one(&state, "trust factor third party belief") {
        assert_eq!(
            factor,
            0.95f64.to_bits(),
            "third-party factor pinned to 0.95"
        );
    }
}

/// An agent-inferred belief is demoted by the shipping `0.85` multiplier.
#[test]
fn agent_inferred_beliefs_report_trust_factor_0_85() {
    let state = state();
    put_belief(
        &state,
        "A",
        "trust factor agent inferred belief",
        SourceType::AgentInferred,
    );

    for factor in factor_bits_for_one(&state, "trust factor agent inferred belief") {
        assert_eq!(
            factor,
            0.85f64.to_bits(),
            "agent-inferred factor pinned to 0.85"
        );
    }
}
