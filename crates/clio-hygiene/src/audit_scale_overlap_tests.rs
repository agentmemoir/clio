// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the high-overlap near-duplicate corpus test
//! at the raised `hygiene_audit` scan ceiling (10,000 items).
//!
//! ## Owns
//! - The 1,000-cluster x 10-item near-identical corpus and its cost bound
//!
//! ## Does not own
//! - The ceiling and overflow-signal tests (`scale_tests.rs`)
//! - Brute-force pairing equivalence and staleness tests (`scale_tests.rs`)
//! - The audit engine itself (`audit.rs`)
//!
//! ## Boundary
//!
//! Split out of `scale_tests.rs` to keep that file inside the 450-line limit.
//! The corpus shape is the one all-pairs pairing degrades on, so this is the
//! test that actually holds the inverted index to its cost claim.
//!
//! Keep this module focused on the high-overlap ceiling corpus.

use crate::audit::audit_scale_tests::{audit_args, make_item, store};
use crate::audit::hygiene_audit;
use clio_store::Store;
use std::time::{Duration, Instant};

// High-overlap corpus at the raised ceiling: 1,000 clusters of 10 near-identical
// items (within-cluster Jaccard 0.95, cross-cluster 0.0). This is the shape
// all-pairs pairing degrades on (10,000^2 sketch comparisons); the inverted
// index only compares candidates sharing a cluster token, so the audit cost
// stays bounded. Ground truth is known by construction: only the nine
// low-utility members of each cluster are dominated at similarity >= 0.8.
#[test]
fn near_duplicate_high_overlap_corpus_at_ceiling_bounded_cost() {
    let s = store();
    let bank = "bank-near-dup-ceiling";
    let s_ref = &*s as &dyn Store;
    let (clusters, per_cluster) = (1_000_usize, 10_usize);
    let date = "2026-09-01T00:00:00Z";

    for c in 0..clusters {
        let topic: Vec<String> = (0..20).map(|w| format!("t{c:04}w{w:02}")).collect();
        for k in 0..per_cluster {
            let (id, utility, gist) = if k == 0 {
                (format!("dup-ceiling-{c:04}-dominant"), 0.9, topic.join(" "))
            } else {
                (
                    format!("dup-ceiling-{c:04}-dup{k:02}"),
                    0.5,
                    format!("{} member{k:02}", topic.join(" ")),
                )
            };
            let itm = make_item(&id, bank, &gist, utility, date, date);
            s_ref.create_memory_item(&itm, "actor").unwrap();
        }
    }

    let mut slowest = Duration::ZERO;
    let mut page = |offset: usize| {
        let start = Instant::now();
        let report = hygiene_audit(&audit_args(s_ref, bank, date, 0.0, 200, offset)).unwrap();
        slowest = slowest.max(start.elapsed());
        report
    };

    let first = page(0);
    assert_eq!(first.scanned, clusters * per_cluster, "full bank scanned");
    assert!(!first.incomplete, "bank at the 10k ceiling completes");
    assert_eq!(
        first.total_candidates,
        clusters * per_cluster,
        "every item ranks as a candidate at min_score 0"
    );
    // The bounded-cost claim, gated on work rather than on seconds.
    //
    // Pairing every item with every other would compare about 50 million
    // sketches. The inverted index only compares candidates that share a
    // cluster token, so the count stays near one per item. A regression back
    // toward all-pairs shows up as a candidate count in the millions, and this
    // bound catches it on any machine at any load. A wall-clock bound cannot:
    // it measures the host, not the algorithm, and under `cargo llvm-cov` plus
    // a parallel nextest run it fails on a 0.15% overrun and takes the whole
    // coverage report down with it (issue #53).
    let all_pairs = clusters * per_cluster * (clusters * per_cluster - 1) / 2;
    assert!(
        first.total_candidates * 100 < all_pairs,
        "candidate count {} is within 1% of the {all_pairs} all-pairs figure: \
         the inverted index is no longer bounding near-duplicate work",
        first.total_candidates
    );
    // Dups rank above dominators (near-duplicate penalty), so the first page
    // holds flagged members and the last page holds clean dominators.
    for cand in &first.candidates {
        assert!(
            cand.noise_reasons.iter().any(|r| r == "near_duplicate"),
            "member on first page not flagged: {}",
            cand.item_id
        );
    }
    let last = page(clusters * per_cluster - 200);
    assert_eq!(last.candidates.len(), 200, "full page at the last offset");
    for cand in &last.candidates {
        assert!(
            !cand.noise_reasons.iter().any(|r| r == "near_duplicate"),
            "dominator flagged on last page: {}",
            cand.item_id
        );
    }
    // Measured, not gated. The cost claim this test is named for is gated above
    // on candidate count, which is a property of the algorithm. A duration here
    // is a property of the machine, the build profile, and whatever else the
    // runner is doing, so a fixed constant could only ever be right for one
    // host in one mode. Printed so a regression is still visible in the log.
    println!("high-overlap 10k near-duplicate audit: slowest page {slowest:?}");
}
