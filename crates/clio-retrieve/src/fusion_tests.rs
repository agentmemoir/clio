// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for weighted-RRF fusion tests: the appendix
//! fixture order, reduction to classic RRF at weight 1, and
//! deterministic weight-change behavior.
//!
//! ## Owns
//! - Pure fusion-math assertions (`rrf_fuse`) with no store or retriever
//! - Per-arm rank and raw-score provenance assertions
//! - The neutral `age_factor` and `trust_factor` initializations on the
//!   `FusedHit`s `rrf_fuse` constructs
//!
//! ## Does not own
//! - End-to-end retriever ordering (`hybrid_tests`) or baseline quality
//!   comparisons (`hybrid_quality_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fusion::{DEFAULT_RRF_K, RankList, rrf_fuse};

fn ids(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn t07_appendix_fixture_order() {
    let dense = ids(&["A", "B", "C"]);
    let lexical = ids(&["B", "D", "A"]);
    let lists = [
        RankList {
            weight: 1.0,
            ids: &dense,
            scores: None,
        },
        RankList {
            weight: 1.0,
            ids: &lexical,
            scores: None,
        },
    ];
    let fused = rrf_fuse(&lists, DEFAULT_RRF_K);
    let order: Vec<&str> = fused.iter().map(|h| h.id.as_str()).collect();
    // Applying RRF_w(d) = w/(k+rank) to the listed ranks gives B > A > D > C.
    // (The appendix prose table labels A and B a tie, but with lexical=[B,D,A]
    // A is lexical rank 3, so B outscores A; the formula is authoritative.)
    assert_eq!(order, vec!["B", "A", "D", "C"]);
    assert!((fused[0].score - (1.0 / 62.0 + 1.0 / 61.0)).abs() < 1e-12);
    assert!((fused[1].score - (1.0 / 61.0 + 1.0 / 63.0)).abs() < 1e-12);
    assert!(fused[1].score > fused[2].score);
    assert!(fused[2].score > fused[3].score);
}

#[test]
fn ranks_are_recorded_per_list() {
    let dense = ids(&["A", "B"]);
    let lexical = ids(&["B", "C"]);
    let lists = [
        RankList {
            weight: 1.0,
            ids: &dense,
            scores: None,
        },
        RankList {
            weight: 1.0,
            ids: &lexical,
            scores: None,
        },
    ];
    let fused = rrf_fuse(&lists, 60);
    let a = fused.iter().find(|h| h.id == "A").expect("A");
    assert_eq!(a.ranks(), vec![Some(1), None]);
    let b = fused.iter().find(|h| h.id == "B").expect("B");
    assert_eq!(b.ranks(), vec![Some(2), Some(1)]);
    let c = fused.iter().find(|h| h.id == "C").expect("C");
    assert_eq!(c.ranks(), vec![None, Some(2)]);
}

#[test]
fn raw_scores_are_recorded_per_list_and_never_change_the_fused_score() {
    let dense = ids(&["A", "B"]);
    let lexical = ids(&["B", "A"]);
    let dense_scores = [0.25_f64, 0.75];
    let lexical_scores = [3.5_f64, 1.5];
    let with_scores = rrf_fuse(
        &[
            RankList {
                weight: 1.0,
                ids: &dense,
                scores: Some(&dense_scores),
            },
            RankList {
                weight: 1.0,
                ids: &lexical,
                scores: Some(&lexical_scores),
            },
        ],
        60,
    );
    let without_scores = rrf_fuse(
        &[
            RankList {
                weight: 1.0,
                ids: &dense,
                scores: None,
            },
            RankList {
                weight: 1.0,
                ids: &lexical,
                scores: None,
            },
        ],
        60,
    );
    // Identical ranks and identical fused scores: raw values are provenance only.
    assert_eq!(with_scores.len(), without_scores.len());
    for (with, without) in with_scores.iter().zip(without_scores.iter()) {
        assert_eq!(with.id, without.id);
        assert_eq!(with.score, without.score);
        assert_eq!(with.ranks(), without.ranks());
    }
    let a = with_scores.iter().find(|h| h.id == "A").expect("A");
    assert_eq!(a.arms[0].score, Some(0.25));
    assert_eq!(a.arms[1].score, Some(1.5));
    let b = with_scores.iter().find(|h| h.id == "B").expect("B");
    assert_eq!(b.arms[0].score, Some(0.75));
    assert_eq!(b.arms[1].score, Some(3.5));
    assert_eq!(a.arms[0].rank, Some(1));
}

#[test]
fn weighted_rrf_reduces_to_classic_when_weights_are_one() {
    let dense = ids(&["A", "B"]);
    let lexical = ids(&["B", "A"]);
    let fused = rrf_fuse(
        &[
            RankList {
                weight: 1.0,
                ids: &dense,
                scores: None,
            },
            RankList {
                weight: 1.0,
                ids: &lexical,
                scores: None,
            },
        ],
        60,
    );
    // Classic Cormack RRF: each id scores 1/61 + 1/62.
    let expected = 1.0 / 61.0 + 1.0 / 62.0;
    assert!((fused[0].score - expected).abs() < 1e-12);
    assert!((fused[1].score - expected).abs() < 1e-12);
    assert_eq!(fused[0].id, "A");
    assert_eq!(fused[1].id, "B");
}

#[test]
fn weight_change_changes_scores_deterministically() {
    let dense = ids(&["A", "B", "C"]);
    let lexical = ids(&["C", "B", "A"]);
    let dense_heavy = rrf_fuse(
        &[
            RankList {
                weight: 1.0,
                ids: &dense,
                scores: None,
            },
            RankList {
                weight: 0.0,
                ids: &lexical,
                scores: None,
            },
        ],
        60,
    );
    let lexical_heavy = rrf_fuse(
        &[
            RankList {
                weight: 0.0,
                ids: &dense,
                scores: None,
            },
            RankList {
                weight: 1.0,
                ids: &lexical,
                scores: None,
            },
        ],
        60,
    );
    assert_eq!(dense_heavy[0].id, "A");
    assert_eq!(lexical_heavy[0].id, "C");
    let a_dense = dense_heavy.iter().find(|h| h.id == "A").expect("A").score;
    let a_lexical = lexical_heavy.iter().find(|h| h.id == "A").expect("A").score;
    assert_ne!(a_dense, a_lexical);
}

#[test]
fn zero_k_is_clamped_and_duplicate_ids_keep_first_rank() {
    let dense = ids(&["A", "A", "B"]);
    let fused = rrf_fuse(
        &[RankList {
            weight: 1.0,
            ids: &dense,
            scores: None,
        }],
        0,
    );
    let a = fused.iter().find(|h| h.id == "A").expect("A");
    assert_eq!(a.ranks(), vec![Some(1)]);
    // k=0 is clamped to k=1, so rank 1 scores 1/(1+1).
    assert!((a.score - 0.5).abs() < 1e-12);
}

#[test]
fn non_finite_or_negative_weight_contributes_zero() {
    let dense = ids(&["A"]);
    let fused = rrf_fuse(
        &[
            RankList {
                weight: f64::NAN,
                ids: &dense,
                scores: None,
            },
            RankList {
                weight: -3.0,
                ids: &dense,
                scores: None,
            },
        ],
        60,
    );
    assert_eq!(fused.len(), 1);
    assert_eq!(fused[0].score, 0.0);
    assert_eq!(fused[0].ranks(), vec![Some(1), Some(1)]);
}

#[test]
fn empty_lists_produce_no_hits() {
    let no_lists = rrf_fuse(&[], 60);
    assert!(no_lists.is_empty(), "{no_lists:?}");
    let empty: Vec<String> = Vec::new();
    let one_empty_list = rrf_fuse(
        &[RankList {
            weight: 1.0,
            ids: &empty,
            scores: None,
        }],
        60,
    );
    assert!(one_empty_list.is_empty(), "{one_empty_list:?}");
}

#[test]
fn rrf_fuse_initializes_the_age_factor_neutral() {
    let dense = ids(&["A", "B"]);
    let fused = rrf_fuse(
        &[
            RankList {
                weight: 1.0,
                ids: &dense,
                scores: None,
            },
            RankList {
                weight: 1.0,
                ids: &dense,
                scores: None,
            },
        ],
        DEFAULT_RRF_K,
    );
    for hit in &fused {
        // Fusion runs before shaping, so the factor starts neutral here and is
        // overwritten only where it is computed.
        assert_eq!(hit.age_factor.to_bits(), 1.0f64.to_bits(), "{}", hit.id);
    }
}

#[test]
fn rrf_fuse_initializes_the_trust_factor_neutral() {
    let dense = ids(&["A", "B"]);
    let fused = rrf_fuse(
        &[
            RankList {
                weight: 1.0,
                ids: &dense,
                scores: None,
            },
            RankList {
                weight: 1.0,
                ids: &dense,
                scores: None,
            },
        ],
        DEFAULT_RRF_K,
    );
    for hit in &fused {
        // The trust multiplier is applied at the shaping site, not by fusion,
        // so every fused hit starts neutral.
        assert_eq!(hit.trust_factor.to_bits(), 1.0f64.to_bits(), "{}", hit.id);
    }
}
