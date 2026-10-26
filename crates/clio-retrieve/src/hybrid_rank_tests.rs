// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the rank-shaping tests of the fusion stage:
//! the exact importance multiplier, the absence of any age term, the graph
//! expansion join, and the two rerank short-circuits.
//!
//! ## Owns
//! - Pure `fuse_candidates` assertions over hand-built legs and item maps, with
//!   no store, no embedder, and no clock
//! - The two `apply_rerank` paths that need no provider: no reranker attached,
//!   and an empty shortlist
//!
//! ## Does not own
//! - The shared shaping fixtures (`hybrid_rank_test_support`)
//! - The fusion formula itself (`fusion_tests`), end-to-end ordering
//!   (`hybrid_tests`), rerank provider binding (`hybrid_rerank_tests`,
//!   `rerank_tests`), or the temporal arm's window membership
//!   (`temporal_arm_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the shaping loop and its rerank short-circuits.

use super::hybrid_rank_test_support::{
    FIXED_NOW, TEN_YEARS_OLD, fixture_item, fixture_items, fixture_legs, ids, score_of,
};
use super::*;
use crate::filter::recency_score;
use crate::fixtures_tests as fx;
use crate::fusion::DEFAULT_RRF_K;
use clio_config::ranking::{DEFAULT_RETENTION_HALF_LIFE_DAYS, RetrievalWeights, TrustRatios};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The retention curve off at a fixed clock: the pre-curve call shape.
///
/// The floor and the half-life are valid on purpose, so nothing but
/// `active = false` keeps the age factor off and a dropped guard cannot pass
/// on the knob check.
const NO_CURVE: RetentionArgs<'static> = RetentionArgs {
    active: false,
    floor: 0.25,
    half_life_days: DEFAULT_RETENTION_HALF_LIFE_DAYS,
    now: FIXED_NOW,
};

/// The unshaped fused score of one fixture id, from the RRF formula.
fn unfused_score(id: &str, fusion_k: usize) -> f64 {
    let half = 0.5;
    match id {
        "A" => half / (fusion_k + 1) as f64,
        "B" => half / (fusion_k + 2) as f64 + half / (fusion_k + 1) as f64,
        "C" => half / (fusion_k + 2) as f64,
        other => panic!("no fused leg rank for {other}"),
    }
}

/// A weight group whose leg weights are equal, so the list weights are 0.5 each
/// and the fused scores stay comparable with [`unfused_score`].
fn weights_with(importance: f64) -> RetrievalWeights {
    RetrievalWeights {
        dense: 1.0,
        lexical: 1.0,
        importance,
    }
}

/// The four fixture items, all stamped at the fixed clock.
fn items_at_now() -> BTreeMap<String, clio_types::MemoryItem> {
    fixture_items([
        ("A", FIXED_NOW),
        ("B", FIXED_NOW),
        ("C", FIXED_NOW),
        ("D", FIXED_NOW),
    ])
}

#[test]
fn shaping_multiplies_the_fused_score_by_the_importance_factor() {
    let (dense, lexical) = fixture_legs();
    let items = items_at_now();
    let importance_weight = 0.25;
    let fused = fuse_candidates(
        &weights_with(importance_weight),
        DEFAULT_RRF_K,
        &dense,
        &lexical,
        &[],
        &items,
        &BTreeSet::new(),
        NO_CURVE,
        TrustRatios::default(),
    );
    assert_eq!(ids(&fused), vec!["B", "A", "C"]);
    for id in ["A", "B", "C"] {
        let admission = items[id].admission_score.expect("fixture admission score");
        // `fused * (1 + w * importance)`, both factors derived from the formula.
        let expected = unfused_score(id, DEFAULT_RRF_K) * (1.0 + importance_weight * admission);
        assert!(
            (score_of(&fused, id) - expected).abs() < 1e-12,
            "{id}: got {}, want {expected}",
            score_of(&fused, id)
        );
    }
}

#[test]
fn the_order_with_no_age_input_is_the_shaped_fused_order() {
    let (dense, lexical) = fixture_legs();
    // Stamps ten years apart: the newest is 2026-01-01 and the oldest 2016-01-01.
    let items = BTreeMap::from([
        ("A".to_owned(), fixture_item("A", 0.8, TEN_YEARS_OLD)),
        ("B".to_owned(), fixture_item("B", 0.2, TEN_YEARS_OLD)),
        ("C".to_owned(), fixture_item("C", 0.5, FIXED_NOW)),
    ]);
    let importance_weight = 0.25;
    let fused = fuse_candidates(
        &weights_with(importance_weight),
        DEFAULT_RRF_K,
        &dense,
        &lexical,
        &[],
        &items,
        &BTreeSet::new(),
        NO_CURVE,
        TrustRatios::default(),
    );
    // Every score is the fused score times its importance factor and nothing
    // else, so the returned order is the shaped order computed here.
    let mut expected_order: Vec<(f64, &str)> = ["A", "B", "C"]
        .iter()
        .map(|id| {
            let admission = items[*id].admission_score.expect("fixture admission score");
            (
                unfused_score(id, DEFAULT_RRF_K) * (1.0 + importance_weight * admission),
                *id,
            )
        })
        .collect();
    expected_order.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("finite"));
    let want: Vec<&str> = expected_order.iter().map(|(_, id)| *id).collect();
    assert_eq!(ids(&fused), want);
}

#[test]
fn the_renormalised_default_matches_the_pre_removal_shaping_formula() {
    let (dense, lexical) = fixture_legs();
    let items = items_at_now();
    // Before the fourth weight was dropped the shaping step multiplied each
    // fused score by `1 + importance * admission + age * recency`, and the group
    // default gave `importance` a third. This test rebuilds the expected score
    // from that formula and demands the shipped code match it byte for byte, so
    // a moved multiplier, a moved fusion, or a reordered list fails here.
    //
    // What it does NOT prove: the removed shape and the current one agree on
    // this fixture only because the age weight is zero, so a nonzero age weight
    // is not covered and nothing here says the removed term was harmless.
    let default = RetrievalWeights::default();
    // The retired age weight, written into the formula rather than dropped, so
    // a reader can see the term that used to be there and why it vanishes here.
    let age_weight = 0.0;
    let fused = fuse_candidates(
        &default,
        DEFAULT_RRF_K,
        &dense,
        &lexical,
        &[],
        &items,
        &BTreeSet::new(),
        NO_CURVE,
        TrustRatios::default(),
    );
    let mut expected_order: Vec<(f64, &str)> = ["A", "B", "C"]
        .iter()
        .map(|id| {
            let item = &items[*id];
            let admission = item.admission_score.expect("fixture admission score");
            // Every fixture item carries the reference stamp, so this recency
            // signal is 1.0, a real nonzero value; only the zero weight cancels it.
            let recency = recency_score(item, FIXED_NOW, 30.0);
            assert!(
                (recency - 1.0).abs() < 1e-12,
                "age 0 gives recency 1.0, got {recency}"
            );
            let multiplier = 1.0 + default.importance * admission + age_weight * recency;
            (unfused_score(id, DEFAULT_RRF_K) * multiplier, *id)
        })
        .collect();
    for (expected, id) in &expected_order {
        assert_eq!(
            score_of(&fused, id).to_bits(),
            expected.to_bits(),
            "{id}: shaped score"
        );
    }
    expected_order.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .expect("finite")
            .then_with(|| a.1.cmp(b.1))
    });
    let want: Vec<&str> = expected_order.iter().map(|(_, id)| *id).collect();
    assert_eq!(ids(&fused), want, "byte-identical order");
    // And the surviving default is exactly the renormalised group.
    assert!((default.dense - 1.0 / 3.0).abs() < 1e-12, "{default:?}");
    assert!((default.lexical - 1.0 / 3.0).abs() < 1e-12, "{default:?}");
    assert!(
        (default.importance - 1.0 / 3.0).abs() < 1e-12,
        "{default:?}"
    );
}

#[test]
fn the_default_leg_weights_are_still_an_even_split() {
    let (dense, lexical) = normalize_leg_weights(&RetrievalWeights::default());
    // The leg weights no longer sum to 1, but they are renormalized against each
    // other, and two equal members still split evenly.
    assert!((dense - 0.5).abs() < 1e-12, "dense leg weight: {dense}");
    assert!(
        (lexical - 0.5).abs() < 1e-12,
        "lexical leg weight: {lexical}"
    );
}

#[test]
fn the_renormalisation_raises_the_multiplier_at_a_fixed_admission_score() {
    let admission = 0.8;
    let multiplier_at = |importance: f64| {
        let fused = fuse_candidates(
            &weights_with(importance),
            DEFAULT_RRF_K,
            &[StageHit {
                id: "A".into(),
                score: 0.9,
            }],
            &[],
            &[],
            &BTreeMap::from([("A".to_owned(), fixture_item("A", admission, FIXED_NOW))]),
            &BTreeSet::new(),
            NO_CURVE,
            TrustRatios::default(),
        );
        let base = unfused_score("A", DEFAULT_RRF_K);
        fused[0].score / base
    };
    let before = multiplier_at(0.25);
    let after = multiplier_at(1.0 / 3.0);
    // 1.20 becomes 1.2667: the weight moved from a quarter to a third while the
    // admission score and the fused score it multiplies stayed put.
    assert!(
        (before - (1.0 + 0.25 * admission)).abs() < 1e-12,
        "got {before}"
    );
    assert!(
        (after - (1.0 + (1.0 / 3.0) * admission)).abs() < 1e-12,
        "got {after}"
    );
    assert!(after > before, "the renormalisation raises the multiplier");
}

#[test]
fn a_hit_whose_item_was_not_fetched_keeps_its_unshaped_score() {
    let (dense, lexical) = fixture_legs();
    let importance_weight = 0.25;
    // "C" is fused from the lexical leg but was not fetched, so the shaping loop
    // has no admission score to read and leaves its fused score alone.
    let items = BTreeMap::from([
        ("A".to_owned(), fixture_item("A", 0.8, FIXED_NOW)),
        ("B".to_owned(), fixture_item("B", 0.2, FIXED_NOW)),
    ]);
    let fused = fuse_candidates(
        &weights_with(importance_weight),
        DEFAULT_RRF_K,
        &dense,
        &lexical,
        &[],
        &items,
        &BTreeSet::new(),
        NO_CURVE,
        TrustRatios::default(),
    );
    assert_eq!(ids(&fused), vec!["B", "A", "C"]);
    assert!(
        (score_of(&fused, "C") - unfused_score("C", DEFAULT_RRF_K)).abs() < 1e-12,
        "unfetched hit: got {}",
        score_of(&fused, "C")
    );
    assert!(
        (score_of(&fused, "A") - unfused_score("A", DEFAULT_RRF_K) * 1.2).abs() < 1e-12,
        "fetched hit is still shaped"
    );
}

#[test]
fn a_zero_importance_weight_reproduces_the_fused_order() {
    let (dense, lexical) = fixture_legs();
    let items = items_at_now();
    // Shaped order with the weight at zero is the unfused order B, A, C, and
    // every score is its unfused value.
    let fused = fuse_candidates(
        &weights_with(0.0),
        DEFAULT_RRF_K,
        &dense,
        &lexical,
        &[],
        &items,
        &BTreeSet::new(),
        NO_CURVE,
        TrustRatios::default(),
    );
    assert_eq!(ids(&fused), vec!["B", "A", "C"]);
    for id in ["A", "B", "C"] {
        assert!(
            (score_of(&fused, id) - unfused_score(id, DEFAULT_RRF_K)).abs() < 1e-12,
            "{id}: got {}",
            score_of(&fused, id)
        );
    }
}

#[test]
fn a_graph_expanded_hit_joins_below_the_best_shaped_score() {
    let (dense, lexical) = fixture_legs();
    let items = items_at_now();
    let importance_weight = 0.25;
    let fused = fuse_candidates(
        &weights_with(importance_weight),
        DEFAULT_RRF_K,
        &dense,
        &lexical,
        &[],
        &items,
        &BTreeSet::from(["D".to_owned()]),
        NO_CURVE,
        TrustRatios::default(),
    );
    assert_eq!(ids(&fused), vec!["B", "A", "C", "D"]);
    // The expanded hit inherits a fraction of the best shaped score and is not
    // itself shaped, so it lands under every shaped hit above it.
    let best = score_of(&fused, "B");
    assert!(
        (score_of(&fused, "D") - best * DEFAULT_GRAPH_DECAY).abs() < 1e-12,
        "got {}",
        score_of(&fused, "D")
    );
    assert!(score_of(&fused, "D") < score_of(&fused, "C"));
}

#[test]
fn a_hit_with_no_reranker_attached_comes_back_unchanged() {
    let (dense, lexical) = fixture_legs();
    let items = items_at_now();
    let fused = fuse_candidates(
        &weights_with(0.25),
        DEFAULT_RRF_K,
        &dense,
        &lexical,
        &[],
        &items,
        &BTreeSet::new(),
        NO_CURVE,
        TrustRatios::default(),
    );
    let mut warnings = Vec::new();
    let (returned, reranked) =
        apply_rerank(None, 60, "query", &items, fused.clone(), &mut warnings);
    assert!(!reranked, "no reranker attached, so rerank did not run");
    assert_eq!(ids(&returned), ids(&fused));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(returned.iter().all(|h| h.rerank_score.is_none()));
}

#[test]
fn an_empty_shortlist_is_not_sent_to_the_reranker() {
    let items = items_at_now();
    let mut warnings = Vec::new();
    // Attached and always failing, so an empty warning list means it was
    // never called at all.
    let failing_provider: Arc<dyn crate::rerank::Reranker> = Arc::new(fx::FailReranker);
    let (returned, reranked) = apply_rerank(
        Some(&failing_provider),
        60,
        "query",
        &items,
        Vec::new(),
        &mut warnings,
    );
    assert!(!reranked, "nothing to rerank");
    assert!(returned.is_empty(), "{returned:?}");
    assert!(
        warnings.is_empty(),
        "the provider was never called, so it cannot warn: {warnings:?}"
    );
}

#[path = "hybrid_rank_tests/rerank_batch_tests.rs"]
mod rerank_batch_tests;
