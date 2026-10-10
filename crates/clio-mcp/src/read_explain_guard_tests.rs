// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the explain projection's value contract and
//! the drift guard: it pins the projected values, sweeps every field of the
//! hit contracts by name against the classification tables, and fails when a
//! hit field is added without a deliberate trace/exclusion decision.
//!
//! ## Owns
//! - The projected per-hit value contract (full and value-less hits)
//! - The key-classification sweep over the `ScoredHit`/`RetrieveHit` field
//!   names, generated from one shared field declaration so the destructuring
//!   and the name list cannot drift, and bound exhaustively so serde-skipped
//!   fields are covered too
//! - The frozen trace/exclusion tables the production projection must match
//!
//! ## Does not own
//! - The wire-level trace binding tests (`read_explain_tests`)
//! - Retrieval behavior or hit construction (`clio-retrieve`)
//! - The projection implementation (`read_explain`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the projection classification contract.

use crate::read_explain::{
    CONTENT_EXCLUDED_HIT_KEYS, SAFE_EXCLUDED_HIT_KEYS, TRACE_HIT_KEYS, explain_hit,
};
use clio_retrieve::{HitScores, ScoredHit};
use clio_types::{EpistemicKind, RetrieveHit, SourceType};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const BANK: &str = "trace-mcp";

/// A name no other field of the trace payload carries.
const ENTITY: &str = "TraceBoundaryName91";

/// A rendering marker no other field of the trace payload carries, so a sweep
/// that finds it in the trace can only have found it in a rendering.
const RENDER_MARKER: &str = "TraceRenderMarker62";

/// A synthetic rendering: a bounded one-string snapshot summary, carrying the
/// marker and nothing real.
fn rendering() -> String {
    let note = RENDER_MARKER;
    format!("{{\"note\":\"{note}\"}}")
}

/// The frozen per-hit explain-trace keys the trace has always carried.
/// Changing this list is a deliberate trace-contract change, not a side effect
/// of adding a field to `ScoredHit`.
const TRACE_HIT_KEYS_EXPECTED: [&str; 6] = [
    "item_id",
    "score",
    "dense_rank",
    "lexical_rank",
    "consolidated",
    "source_ids",
];

/// The frozen content-derived hit keys that must never reach the trace.
const CONTENT_EXCLUDED_EXPECTED: [&str; 4] = ["gist", "context", "entities", "exact_render"];

/// The frozen PII-safe hit keys deliberately kept out of the trace.
/// `temporal_rank` is the temporal arm's payload rank: it stays trace-absent so
/// the per-hit trace keeps its frozen shape; the payload's hits and the trace's
/// conditional `temporal_candidates` counter carry the arm's observability.
const SAFE_EXCLUDED_EXPECTED: [&str; 12] = [
    "bank_id",
    "kind",
    "category",
    "episodic_type",
    "epistemic_kind",
    "source_type",
    "confidence",
    "requires_confidence_check",
    "scores",
    "snapshot_ref",
    "entity_match",
    "temporal_rank",
];

/// Every classification the drift guard understands.
#[derive(Debug, PartialEq, Eq)]
enum Classification {
    /// The key is emitted in the explain trace.
    TraceVisible,
    /// The key is content-derived and must never enter the trace.
    ContentExcluded,
    /// The key is PII-safe but deliberately absent from the trace today.
    SafeExcluded,
}

/// Classify one serialized hit key against the production tables.
fn classify(key: &str) -> Option<Classification> {
    if TRACE_HIT_KEYS.contains(&key) {
        return Some(Classification::TraceVisible);
    }
    if CONTENT_EXCLUDED_HIT_KEYS.contains(&key) {
        return Some(Classification::ContentExcluded);
    }
    if SAFE_EXCLUDED_HIT_KEYS.contains(&key) {
        return Some(Classification::SafeExcluded);
    }
    None
}

/// A set of key names for set-equality assertions.
fn set_of<const N: usize>(keys: [&str; N]) -> BTreeSet<String> {
    keys.map(String::from).into_iter().collect()
}

/// The object keys of one JSON object, as a set.
fn keys(value: &Value) -> BTreeSet<String> {
    value.as_object().expect("object").keys().cloned().collect()
}

/// A hit with every optional field present, so the serialized key sweep sees
/// the full `ScoredHit` surface.
fn full_hit() -> ScoredHit {
    ScoredHit {
        hit: RetrieveHit {
            item_id: "full-hit".into(),
            bank_id: BANK.into(),
            kind: "belief".into(),
            category: Some("schema".into()),
            episodic_type: Some("task".into()),
            epistemic_kind: EpistemicKind::Belief,
            source_type: Some(SourceType::UserStated),
            confidence: Some(0.5),
            gist: Some("full gist".into()),
            context: Some("full context".into()),
            exact_render: Some(rendering()),
            requires_confidence_check: true,
        },
        score: 0.75,
        scores: HitScores {
            final_score: 0.75,
            reranker: Some(0.5),
            semantic: Some(0.25),
            keyword: Some(1.5),
            age_factor: 1.0,
            trust_factor: 1.0,
        },
        dense_rank: Some(2),
        lexical_rank: Some(3),
        temporal_rank: Some(4),
        snapshot_ref: Some("full-hit".into()),
        entities: vec![ENTITY.into()],
        entity_match: true,
        consolidated: true,
        source_ids: vec!["src-1".into()],
    }
}

/// Declares every field of the frozen `ScoredHit` and flattened `RetrieveHit`
/// contracts exactly once, then re-expands the single list through `$mac`.
/// Both guard artifacts — the `..`-free destructuring pattern and the swept
/// name list — are generated from this one declaration, so a field cannot be
/// mentioned in one artifact and silently dropped from the other. `inner` are
/// the fields of the flattened `RetrieveHit` (`ScoredHit.hit`); `outer` are
/// the fields of `ScoredHit` itself.
macro_rules! with_hit_fields {
    ($mac:ident) => {
        $mac! {
            inner {
                item_id, bank_id, kind, category, episodic_type, epistemic_kind,
                source_type, confidence, gist, context, exact_render,
                requires_confidence_check
            }
            outer {
                score, scores, dense_rank, lexical_rank, temporal_rank,
                snapshot_ref, entities, entity_match, consolidated, source_ids
            }
        }
    };
}

/// Generates `hit_field_names` from the single field declaration: the
/// destructuring has no `..` and no wildcard, so adding a field to either
/// struct fails compilation here, and the returned names come from the same
/// list the pattern was built from. The names are the serde keys: neither
/// struct renames a field, and `ScoredHit.hit` is `#[serde(flatten)]`, so the
/// inner fields serialize under their own names.
macro_rules! make_hit_field_names {
    (inner { $($inner:ident),* } outer { $($outer:ident),* }) => {
        /// Every field of the frozen `ScoredHit` and flattened `RetrieveHit`
        /// contracts, bound by name from the one shared declaration, so the
        /// classification sweep covers fields serde never serializes too
        /// (for example a `#[serde(skip)]` field).
        fn hit_field_names(hit: &ScoredHit) -> Vec<&'static str> {
            let ScoredHit {
                hit: RetrieveHit { $($inner: _),* },
                $($outer: _),*
            } = hit;
            vec![$(stringify!($inner),)* $(stringify!($outer),)*]
        }
    };
}

with_hit_fields!(make_hit_field_names);

/// A hit with no optional value set at all.
fn sparse_hit() -> ScoredHit {
    let mut hit = full_hit();
    hit.hit.item_id = "sparse-hit".into();
    hit.score = 0.0;
    hit.scores = HitScores {
        final_score: 0.0,
        reranker: None,
        semantic: None,
        keyword: None,
        age_factor: 1.0,
        trust_factor: 1.0,
    };
    hit.dense_rank = None;
    hit.lexical_rank = None;
    hit.temporal_rank = None;
    hit.snapshot_ref = None;
    hit.hit.exact_render = None;
    hit.entities = Vec::new();
    hit.entity_match = false;
    hit.consolidated = false;
    hit.source_ids = Vec::new();
    hit
}

/// The shared projection emits exactly the frozen per-hit values, and a
/// value-less hit keeps every key with `null`/empty values.
#[test]
fn projection_reproduces_the_frozen_hit_contract() {
    assert_eq!(
        serde_json::to_value(explain_hit(&full_hit())).expect("serialize projection"),
        json!({
            "item_id": "full-hit",
            "score": 0.75,
            "dense_rank": 2,
            "lexical_rank": 3,
            "consolidated": true,
            "source_ids": ["src-1"]
        })
    );
    assert_eq!(
        serde_json::to_value(explain_hit(&sparse_hit())).expect("serialize projection"),
        json!({
            "item_id": "sparse-hit",
            "score": 0.0,
            "dense_rank": null,
            "lexical_rank": null,
            "consolidated": false,
            "source_ids": []
        })
    );
}

/// Every field of the hit contracts is classified, and the projection emits
/// exactly the trace-visible set. The sweep walks the struct field NAMES, not
/// the serialized keys, so a field serde skips cannot hide from it.
///
/// This test is also the guard against a reintroduced `..`/wildcard in the
/// `explain_hit` pattern: the one thing `..` makes silent is dropping a field
/// from the projection, and a field classified `TraceVisible` but dropped
/// fails both the per-field presence assertion and the allowlist equality
/// below.
#[test]
fn every_hit_field_is_classified_and_matches_the_projection() {
    assert_eq!(
        classify("brand_new_non_content_field"),
        None,
        "a simulated new field has no classification, exactly as a real one would"
    );
    let hit = full_hit();
    let names = hit_field_names(&hit);
    let serialized_keys = keys(&serde_json::to_value(&hit).expect("serialize full hit"));
    let projected = serde_json::to_value(explain_hit(&hit)).expect("serialize projection");
    let projected_keys = keys(&projected);

    // The swept names and the serialized surface coincide for the fully
    // populated fixture: nothing is renamed and every field serializes, so a
    // typo or omission in either list fails here.
    let name_set: BTreeSet<String> = names.iter().map(|name| String::from(*name)).collect();
    assert_eq!(
        name_set, serialized_keys,
        "the swept field names and the serialized keys diverged: fix the fixture or the name list"
    );
    for key in TRACE_HIT_KEYS
        .iter()
        .chain(&CONTENT_EXCLUDED_HIT_KEYS)
        .chain(&SAFE_EXCLUDED_HIT_KEYS)
    {
        assert!(
            serialized_keys.contains(*key),
            "classification entry `{key}` is not a serialized hit key; fix the fixture or drop the stale entry"
        );
    }
    for name in &names {
        let classification = classify(name);
        assert!(
            classification.is_some(),
            "hit field `{name}` is unclassified: add it to TRACE_HIT_KEYS if it belongs in the trace, or to an exclusion set with a reason"
        );
        assert_eq!(
            classification == Some(Classification::TraceVisible),
            projected_keys.contains(*name),
            "classification and projection disagree for `{name}`"
        );
    }
    assert_eq!(
        projected_keys,
        set_of(TRACE_HIT_KEYS),
        "the projection must emit exactly the declared allowlist"
    );
    for excluded in CONTENT_EXCLUDED_HIT_KEYS {
        assert!(
            !projected_keys.contains(excluded),
            "content key `{excluded}` must never reach the trace"
        );
    }
}

/// The bounded rendering is content, and all three enforcement mechanisms
/// refuse it the trace, each asserted separately.
///
/// The projection has three independent gates and a rendering only stays out
/// while all three hold. If any one of them is removed the others do not cover
/// it, so the sweep names the mechanism it is exercising rather than asserting
/// the outcome three times:
///
/// 1. The guard macro's exhaustive `RetrieveHit { $($inner: _),* }` construction
///    — a new `RetrieveHit` field does not compile until it is deliberately
///    classified, while `explain_hit`'s `..`-free destructuring is that gate for a
///    `ScoredHit` field, because it binds the inner hit as a whole. A
///    compile-time gate cannot be asserted from a test, so the sweep asserts the
///    property it protects: an allowlisted field carries only its own value, so no
///    field can smuggle the rendering in under an allowed key.
/// 2. The drift sweep's classification tables — the field must be
///    `ContentExcluded`, never `TraceVisible`.
/// 3. The projection allowlist — the projected keys are exactly
///    `TRACE_HIT_KEYS`, so there is no slot the rendering could occupy.
#[test]
fn exact_render_is_content_and_all_three_mechanisms_refuse_it_the_trace() {
    let hit = full_hit();
    let projected = serde_json::to_value(explain_hit(&hit)).expect("serialize projection");
    let projected_keys = keys(&projected);

    // Mechanism 2: the field is classified content, not trace-visible.
    assert_eq!(
        classify("exact_render"),
        Some(Classification::ContentExcluded),
        "a bounded rendering of stored content is content"
    );

    // Mechanism 3: the projection emits exactly the allowlist, and every
    // content key is absent from it.
    assert_eq!(
        projected_keys,
        set_of(TRACE_HIT_KEYS),
        "the projection must emit exactly the declared allowlist"
    );
    for excluded in CONTENT_EXCLUDED_HIT_KEYS {
        assert!(
            !projected_keys.contains(excluded),
            "content key `{excluded}` must never reach the trace"
        );
    }

    // Mechanism 1, at the layer a test can observe: the projected `item_id` is
    // the id and nothing else, even though the same hit also carries a
    // rendering. A projection that smuggled the rendering into an allowlisted
    // field would keep the key set above intact and fail only here.
    assert_eq!(
        projected["item_id"],
        json!("full-hit"),
        "an allowlisted field must carry its own value, never the rendering"
    );

    // The parent's negative: the rendering's text is absent from the whole
    // serialized trace hit, not merely from its key set.
    let serialized = serde_json::to_string(&projected).expect("serialize projection");
    assert!(
        !serialized.contains(RENDER_MARKER),
        "the rendering's text must not appear anywhere in the trace hit: {serialized}"
    );
    assert!(
        !serialized.contains(ENTITY),
        "no content text may appear anywhere in the trace hit: {serialized}"
    );

    // The fixture is the premise of the negative above: an absent rendering
    // would make the sweep pass for the wrong reason.
    assert_eq!(
        hit.hit.exact_render.as_deref(),
        Some(rendering().as_str()),
        "the fixture must actually populate the rendering, or the sweep proves nothing"
    );
}

/// The production classification tables stay identical to the frozen trace
/// contract. Changing what the trace shows requires editing both sides in one
/// deliberate change.
#[test]
fn classification_tables_match_the_frozen_trace_contract() {
    assert_eq!(
        set_of(TRACE_HIT_KEYS),
        set_of(TRACE_HIT_KEYS_EXPECTED),
        "the trace allowlist drifted from the frozen per-hit contract"
    );
    assert_eq!(
        set_of(CONTENT_EXCLUDED_HIT_KEYS),
        set_of(CONTENT_EXCLUDED_EXPECTED),
        "the content-exclusion table drifted"
    );
    assert_eq!(
        set_of(SAFE_EXCLUDED_HIT_KEYS),
        set_of(SAFE_EXCLUDED_EXPECTED),
        "the deliberate-exclusion table drifted"
    );
}
