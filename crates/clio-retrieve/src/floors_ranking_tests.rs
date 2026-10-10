// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the post-ranking score floors over the real
//! retrieval path: the `reranker` floor with rerank on, off, and failed
//! (fail-open), and the `final` floor on the returned page including the
//! no-backfill rule, the budget, and the empty page.
//!
//! ## Owns
//! - `reranker` floor evidence in all three rerank states
//! - `final` floor: returned-set filtering, no backfill, budget, empty page
//!
//! ## Does not own
//! - The arm-local `semantic` / `keyword` floors
//!   (`floors_integration_tests`), and the predicates themselves (`floors_tests`)
//! - The entity-inclusion controls (`entity_controls_tests`)
//! - Binding-level payloads (`clio-mcp` / `clio` tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the floors that run after fusion.

use crate::fixtures_tests as fx;
use crate::floors::floors_integration_tests::{hit_ids, request};
use clio_types::ErrorCode;
use std::sync::Arc;

#[test]
fn a_reranker_floor_keeps_every_hit_when_rerank_is_off() {
    let store = fx::store();
    for id in ["A", "B"] {
        let entry = fx::item(id, "bank", &format!("alpha memory {id}"));
        fx::put_lexical(
            &store,
            &entry,
            &format!("alpha memory {id} padding padding"),
        );
    }
    let retriever = fx::retriever(&store);
    let mut req = request("bank", "alpha");
    req.limit = 5;
    req.min_scores.reranker = Some(0.5);
    let out = retriever.retrieve(&req).expect("floored retrieve");

    assert!(!out.reranked, "no reranker is configured");
    assert_eq!(hit_ids(&out), vec!["A".to_owned(), "B".to_owned()]);
    for hit in &out.hits {
        assert_eq!(
            hit.scores.reranker, None,
            "no rerank score exists, so the floor removed nothing: {hit:?}"
        );
    }
}

#[test]
fn a_reranker_floor_drops_scored_candidates_below_it() {
    let store = fx::store();
    for id in ["A", "B"] {
        let entry = fx::item(id, "bank", &format!("alpha memory {id}"));
        fx::put_lexical(
            &store,
            &entry,
            &format!("alpha memory {id} padding padding"),
        );
    }
    let mut retriever = fx::retriever(&store);
    retriever.set_reranker(Arc::new(fx::ReverseReranker));
    let mut req = request("bank", "alpha");
    req.limit = 5;
    req.min_scores.reranker = Some(0.85);
    let out = retriever.retrieve(&req).expect("floored retrieve");

    assert!(out.reranked, "the reranker ran");
    let ids = hit_ids(&out);
    assert_eq!(
        ids.len(),
        1,
        "only the above-floor candidate survives: {ids:?}"
    );
    let hit = &out.hits[0];
    assert!(
        hit.scores.reranker.is_some_and(|score| score >= 0.85),
        "the survivor clears the floor: {hit:?}"
    );
}

#[test]
fn a_reranker_floor_is_a_no_op_when_the_reranker_fails_open() {
    let store = fx::store();
    for id in ["A", "B"] {
        let entry = fx::item(id, "bank", &format!("alpha memory {id}"));
        fx::put_lexical(
            &store,
            &entry,
            &format!("alpha memory {id} padding padding"),
        );
    }
    let mut retriever = fx::retriever(&store);
    retriever.set_reranker(Arc::new(fx::FailReranker));
    let mut req = request("bank", "alpha");
    req.limit = 5;
    req.min_scores.reranker = Some(0.5);
    let out = retriever.retrieve(&req).expect("floored retrieve");

    assert!(!out.reranked, "the rerank error kept the fused order");
    assert_eq!(hit_ids(&out), vec!["A".to_owned(), "B".to_owned()]);
    for hit in &out.hits {
        assert_eq!(hit.scores.reranker, None, "{hit:?}");
    }
}

#[test]
fn the_final_floor_filters_the_page_without_backfilling() {
    let store = fx::store();
    for id in ["A", "B", "C"] {
        let entry = fx::item(id, "bank", &format!("alpha memory {id}"));
        fx::put_lexical(
            &store,
            &entry,
            &format!("alpha memory {id} padding padding"),
        );
    }
    let retriever = fx::retriever(&store);

    // Rank the three hits with an unfloored call, then floor between the two
    // best: the third candidate is the one a backfill would wrongly promote.
    let mut probe = request("bank", "alpha");
    probe.limit = 3;
    let ranked = retriever.retrieve(&probe).expect("unfloored retrieve");
    assert_eq!(hit_ids(&ranked).len(), 3, "all three are candidates");
    let (first, second, third) = (
        ranked.hits[0].score,
        ranked.hits[1].score,
        ranked.hits[2].score,
    );
    assert!(
        first > second && second > third,
        "fixture scores must be distinct: {first} {second} {third}"
    );

    let mut req = request("bank", "alpha");
    req.limit = 2;
    req.min_scores.final_score = Some(f64::midpoint(first, second));
    let out = retriever.retrieve(&req).expect("floored retrieve");

    assert_eq!(
        hit_ids(&out),
        vec![ranked.hits[0].hit.item_id.clone()],
        "one hit clears the floor; the freed slot is not refilled from rank 3"
    );
    assert!(
        out.estimated_tokens < ranked.estimated_tokens,
        "the returned page is smaller than the unfloored page"
    );
    for hit in &out.hits {
        assert!(
            hit.score >= f64::midpoint(first, second),
            "every returned hit clears the floor: {hit:?}"
        );
    }
}

#[test]
fn the_final_floor_reads_the_curve_demoted_score() {
    let store = fx::store();
    let recent = fx::item("RECENT", "bank", "alpha memory recent");
    fx::put_lexical(&store, &recent, "alpha memory recent padding padding");
    // Ten years before the fixed clock: the age factor converges to within
    // epsilon of the request's curve floor, so the curve demotes this hit by
    // the full amount.
    let mut aged = fx::item("AGED", "bank", "alpha memory aged");
    aged.created_at = "2016-01-01T00:00:00Z".into();
    aged.updated_at = "2016-01-01T00:00:00Z".into();
    aged.valid_from = "2016-01-01T00:00:00Z".into();
    fx::put_lexical(&store, &aged, "alpha memory aged padding padding");
    let retriever = fx::retriever(&store);

    // Curve off: the aged hit's pre-curve fused value.
    let mut off = request("bank", "alpha");
    off.limit = 5;
    let uncurved = retriever.retrieve(&off).expect("uncurved retrieve");
    assert_eq!(hit_ids(&uncurved).len(), 2, "both hits are candidates");
    let pre_curve = uncurved
        .hits
        .iter()
        .find(|hit| hit.hit.item_id == "AGED")
        .expect("AGED hit")
        .score;

    // Curve on: the same hit's demoted value.
    let mut on = request("bank", "alpha");
    on.limit = 5;
    on.retention_curve = true;
    let curved = retriever.retrieve(&on).expect("curved retrieve");
    let demoted = curved
        .hits
        .iter()
        .find(|hit| hit.hit.item_id == "AGED")
        .expect("the curve keeps AGED on the page")
        .score;
    assert!(
        demoted < pre_curve,
        "the curve did not demote AGED below {pre_curve}: {demoted}"
    );

    // A floor strictly between the two values: the curve-on call reads the
    // demoted value and drops the hit, the curve-off control keeps it.
    let floor = f64::midpoint(demoted, pre_curve);
    let mut on_floor = request("bank", "alpha");
    on_floor.limit = 5;
    on_floor.retention_curve = true;
    on_floor.min_scores.final_score = Some(floor);
    let floored = retriever.retrieve(&on_floor).expect("floored retrieve");
    assert_eq!(
        hit_ids(&floored),
        vec!["RECENT".to_owned()],
        "the floor excludes the demoted hit and keeps the recent one"
    );

    let mut off_floor = request("bank", "alpha");
    off_floor.limit = 5;
    off_floor.min_scores.final_score = Some(floor);
    let control = retriever.retrieve(&off_floor).expect("control retrieve");
    let mut ids = hit_ids(&control);
    ids.sort();
    assert_eq!(
        ids,
        vec!["AGED".to_owned(), "RECENT".to_owned()],
        "without the curve the aged hit clears the same floor"
    );
}

#[test]
fn the_final_floor_never_exceeds_the_budget() {
    let store = fx::store();
    for id in ["A", "B", "C"] {
        let entry = fx::item(id, "bank", &format!("alpha memory {id}"));
        fx::put_lexical(
            &store,
            &entry,
            &format!("alpha memory {id} padding padding"),
        );
    }
    let retriever = fx::retriever(&store);
    let mut req = request("bank", "alpha");
    req.limit = 5;
    req.budget_tokens = Some(24);
    req.min_scores.final_score = Some(0.0001);
    let out = retriever.retrieve(&req).expect("floored retrieve");

    assert!(!out.hits.is_empty(), "a small floor keeps the page");
    assert!(
        out.estimated_tokens <= 24,
        "the budget still holds: {}",
        out.estimated_tokens
    );
}

#[test]
fn an_empty_page_after_flooring_is_returned_as_empty() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "alpha memory A");
    fx::put_lexical(&store, &entry, "alpha memory token");
    let retriever = fx::retriever(&store);

    let mut req = request("bank", "alpha");
    req.min_scores.final_score = Some(MAX_UNREACHABLE_FLOOR);
    let out = retriever.retrieve(&req).expect("floored retrieve");

    assert!(out.hits.is_empty(), "an empty page is a valid answer");
    assert_eq!(out.estimated_tokens, 0, "no hit, no tokens");
    assert_eq!(
        out.fused_candidates, 1,
        "the candidate existed; the floor removed it"
    );
}

/// A floor no real fused score reaches, used to empty the page on purpose.
const MAX_UNREACHABLE_FLOOR: f64 = crate::floors::MAX_FLOOR;

#[test]
fn an_invalid_floor_is_rejected_by_the_retrieve_entry_point() {
    let store = fx::store();
    let retriever = fx::retriever(&store);

    let mut req = request("bank", "alpha");
    req.min_scores.semantic = Some(1.5);
    assert_eq!(
        retriever
            .retrieve(&req)
            .expect_err("an unreachable semantic floor is rejected")
            .code,
        ErrorCode::OutOfRange
    );

    let mut req = request("bank", "alpha");
    req.min_scores.keyword = Some(-1.0);
    assert_eq!(
        retriever
            .retrieve(&req)
            .expect_err("a negative keyword floor is rejected")
            .code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn a_contradictory_entity_control_pair_is_rejected_by_retrieve() {
    let store = fx::store();
    let retriever = fx::retriever(&store);
    let mut req = request("bank", "alpha");
    req.include_entities = false;
    req.entity_summary = true;
    let error = retriever
        .retrieve(&req)
        .expect_err("a summary without entity names is contradictory");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("entity_summary"), "{error}");
}
