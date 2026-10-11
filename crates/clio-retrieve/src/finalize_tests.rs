// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for finalize-step tests: `budget_tokens` trimming
//! of returned hits, the deliberate at-least-one-hit policy, and the carried
//! per-hit score factors.
//!
//! ## Owns
//! - Budget-trim assertions for the final result mapping, including the
//!   first-hit-over-budget policy and its documented budget consequence
//! - The assertions that `scores.age_factor` and `scores.trust_factor` report
//!   the values carried on the fused hit, never ones recomputed here
//! - The `include_exact` control's observable effect on the assembled hit: no
//!   rendering without it, a whole rendering with it, and none for an
//!   over-cap snapshot
//!
//! ## Does not own
//! - Candidate generation, fusion, or rerank behavior (`hybrid_tests`,
//!   `fusion_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::dedup::DedupMeta;
use crate::finalize::{FinalizeArgs, finalize};
use crate::fixtures_tests as fx;
use crate::fusion::{ArmSignal, FusedHit};
use crate::render::EXACT_RENDER_MAX_BYTES;
use crate::types::{RetrieveMetrics, RetrieveRequest, ScoredHit};
use clio_config::retention::RecallScope;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;

fn request(bank: &str, query: &str) -> RetrieveRequest {
    RetrieveRequest::new(bank, query)
}

/// Finalize one hand-built hit whose item carries `snapshot`.
///
/// The item is built in memory and passed straight to `finalize`, so the test
/// needs no store, no clock, no port, and no path: it depends on nothing but
/// its own arguments and is therefore order-independent and repeatable.
fn finalize_one(snapshot: Option<Value>, include_exact: bool) -> ScoredHit {
    let mut req = request("bank", "q");
    req.include_exact = include_exact;
    let mut item = fx::item("H", "bank", "fresh gist");
    item.snapshot = snapshot;
    let items = BTreeMap::from([("H".to_owned(), item)]);
    let fused = vec![FusedHit {
        id: "H".into(),
        score: 0.75,
        arms: vec![ArmSignal::default(); 2],
        rerank_score: None,
        age_factor: 1.0,
        trust_factor: 1.0,
    }];
    let out = finalize(FinalizeArgs {
        req: &req,
        items: &items,
        dense_rank: &BTreeMap::new(),
        lexical_rank: &BTreeMap::new(),
        fused,
        dense_candidates: 0,
        lexical_candidates: 0,
        temporal_candidates: None,
        reranked: false,
        graph_expanded: 0,
        metrics: RetrieveMetrics {
            dense_ms: 0,
            lexical_ms: 0,
            expand_ms: 0,
            rerank_ms: 0,
            total_ms: 0,
        },
        warnings: Vec::new(),
        floor_binding: Vec::new(),
        dedup: DedupMeta {
            scope: RecallScope::Full,
            scope_empty: false,
            candidates_considered: 1,
            suppressed_raw_ids: Vec::new(),
            near_dup_suppressed: 0,
            exact_dup_suppressed: 0,
        },
        backfill_baseline: vec!["H".to_owned()],
    });
    out.hits.into_iter().next().expect("one hit was finalized")
}

#[test]
fn a_request_that_does_not_ask_for_a_rendering_leaves_the_payload_untouched() {
    let snapshot = json!({"unit_id": "u", "hub_id": "h", "neighbor_count": 0, "neighbor_ids": []});
    let asked = finalize_one(Some(snapshot.clone()), false);
    assert!(
        asked.hit.exact_render.is_none(),
        "no rendering without the request control: {:?}",
        asked.hit.exact_render
    );
    let asked_json = serde_json::to_value(&asked).expect("serialize the hit");

    // The only difference a request that asks may make is the added key, so
    // dropping that key must give back the not-asked payload byte for byte.
    let rendered = finalize_one(Some(snapshot), true);
    let mut rendered_json = serde_json::to_value(&rendered).expect("serialize the hit");
    assert_eq!(
        rendered_json
            .as_object_mut()
            .expect("a hit is an object")
            .remove("exact_render"),
        Some(json!(
            r#"{"hub_id":"h","neighbor_count":0,"neighbor_ids":[],"unit_id":"u"}"#
        )),
        "the asked-for payload differs from the not-asked one by the rendering alone"
    );
    assert_eq!(
        rendered_json, asked_json,
        "every other key is byte-identical whether or not the request asked"
    );
}

#[test]
fn a_request_that_does_not_ask_serializes_no_rendering_key() {
    let hit = finalize_one(Some(json!({"key": "value"})), false);
    let json_text = serde_json::to_string(&hit).expect("serialize the hit");
    assert!(
        !json_text.contains("exact_render"),
        "the field is absent, not empty: {json_text}"
    );
}

#[test]
fn an_asked_for_snapshot_within_the_cap_carries_its_whole_rendering() {
    let snapshot = fx::snapshot_of_bytes(EXACT_RENDER_MAX_BYTES);
    let hit = finalize_one(Some(snapshot.clone()), true);
    let rendering = hit
        .hit
        .exact_render
        .clone()
        .expect("a 512-byte snapshot is within the cap");
    assert_eq!(
        rendering.len(),
        EXACT_RENDER_MAX_BYTES,
        "the whole rendering is carried"
    );
    assert_eq!(
        rendering,
        serde_json::to_string(&snapshot).expect("serialize the snapshot"),
        "the rendering is the item's own snapshot"
    );
    let json_text = serde_json::to_string(&hit).expect("serialize the hit");
    assert!(json_text.contains("\"exact_render\":"), "{json_text}");
}

#[test]
fn an_asked_for_snapshot_over_the_cap_carries_no_rendering() {
    let snapshot = fx::snapshot_of_bytes(EXACT_RENDER_MAX_BYTES + 1);
    let hit = finalize_one(Some(snapshot), true);
    assert!(
        hit.hit.exact_render.is_none(),
        "an over-cap snapshot carries nothing, never a prefix: {:?}",
        hit.hit.exact_render
    );
    let json_text = serde_json::to_string(&hit).expect("serialize the hit");
    assert!(!json_text.contains("exact_render"), "{json_text}");
}

#[test]
fn an_asked_for_item_without_a_snapshot_carries_no_rendering() {
    let hit = finalize_one(None, true);
    assert!(
        hit.hit.exact_render.is_none(),
        "there is no snapshot to render: {:?}",
        hit.hit.exact_render
    );
    assert!(
        hit.snapshot_ref.is_none(),
        "the same item has no snapshot reference either"
    );
}

#[test]
fn budget_tokens_trims_returned_hits() {
    let store = fx::store();
    for id in ["A", "B"] {
        let entry = fx::item(id, "bank", "shared");
        fx::put_lexical(
            &store,
            &entry,
            &format!("shared token {id} padding padding"),
        );
    }
    let retriever = fx::retriever(&store);
    let mut req = request("bank", "shared");
    req.budget_tokens = Some(8);
    let out = retriever.retrieve(&req).expect("retrieve");
    assert!(out.hits.len() < 2, "hits: {:?}", out.hits.len());
    assert!(out.estimated_tokens <= 8);
}

#[test]
fn budget_tokens_always_returns_first_hit_even_over_budget() {
    let store = fx::store();
    // finalize budgets the hit's lexical source text (gist + snapshot), so the
    // oversized hit must be oversized there, not only in its search document.
    let oversized = fx::item("BIG", "bank", &"payload ".repeat(120));
    fx::put_lexical(&store, &oversized, "payload token");
    let small = fx::item("S", "bank", "small memory");
    fx::put_lexical(&store, &small, "tiny doc");
    let retriever = fx::retriever(&store);
    let mut req = request("bank", "payload");
    req.budget_tokens = Some(8);
    let out = retriever.retrieve(&req).expect("retrieve");
    assert_eq!(out.hits.len(), 1, "hits: {:?}", out.hits.len());
    assert_eq!(out.hits[0].hit.item_id, "BIG");
    assert!(
        out.estimated_tokens > 8,
        "first hit is admitted even when it alone exceeds the budget \
         (documented at-least-one-hit policy; compose re-truncates on injection)"
    );
}

#[test]
fn finalize_reports_the_carried_age_factor_without_recomputing() {
    let req = request("bank", "q");
    // The fixture item's stamp is fresh, so a factor recomputed from it at
    // assembly time would be exactly `1.0`; the hand-built hit carries a value
    // that no recompute could produce.
    let item = fx::item("H", "bank", "fresh gist");
    let items = BTreeMap::from([("H".to_owned(), item)]);
    let fused = vec![FusedHit {
        id: "H".into(),
        score: 0.75,
        arms: vec![ArmSignal::default(); 2],
        rerank_score: None,
        age_factor: 0.375,
        trust_factor: 0.5,
    }];
    let out = finalize(FinalizeArgs {
        req: &req,
        items: &items,
        dense_rank: &BTreeMap::new(),
        lexical_rank: &BTreeMap::new(),
        fused,
        dense_candidates: 0,
        lexical_candidates: 0,
        temporal_candidates: None,
        reranked: false,
        graph_expanded: 0,
        metrics: RetrieveMetrics {
            dense_ms: 0,
            lexical_ms: 0,
            expand_ms: 0,
            rerank_ms: 0,
            total_ms: 0,
        },
        warnings: Vec::new(),
        floor_binding: Vec::new(),
        dedup: DedupMeta {
            scope: RecallScope::Full,
            scope_empty: false,
            candidates_considered: 1,
            suppressed_raw_ids: Vec::new(),
            near_dup_suppressed: 0,
            exact_dup_suppressed: 0,
        },
        backfill_baseline: vec!["H".to_owned()],
    });
    let hit = out
        .hits
        .iter()
        .find(|h| h.hit.item_id == "H")
        .expect("H hit");
    assert_eq!(
        hit.scores.age_factor.to_bits(),
        0.375f64.to_bits(),
        "the reported factor is the carried one: {}",
        hit.scores.age_factor
    );
    assert_ne!(
        hit.scores.age_factor.to_bits(),
        1.0f64.to_bits(),
        "the carried value, not the neutral default, is reported"
    );
    // The trust factor is carried the same way: the fixture item is a fact, so
    // a recompute would give the neutral `1.0`; the hand-built hit carries 0.5.
    assert_eq!(
        hit.scores.trust_factor.to_bits(),
        0.5f64.to_bits(),
        "the reported factor is the carried one: {}",
        hit.scores.trust_factor
    );
    let value = json!(hit.scores);
    assert_eq!(value["age_factor"], json!(0.375), "serialized as a number");
    assert_eq!(value["trust_factor"], json!(0.5), "serialized as a number");
}
