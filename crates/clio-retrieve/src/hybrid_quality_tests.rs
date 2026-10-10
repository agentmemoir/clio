// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the labeled retrieval-quality fixture:
//! comparing hybrid recall against dense-only and lexical-only baselines.
//!
//! ## Owns
//! - Recall@1 averaged over a two-query labeled fixture (labeled-metric check),
//!   asserting hybrid >= each single-leg baseline
//!
//! ## Does not own
//! - Fusion math correctness (`fusion_tests`) or orchestration edge cases
//!   (`hybrid_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fixtures_tests as fx;
use crate::types::{RetrieveOutcome, RetrieveRequest};
use clio_config::ranking::RetrievalWeights;
use std::sync::Arc;

fn recall_at_1(out: &RetrieveOutcome, relevant_id: &str) -> f64 {
    match out.hits.first() {
        Some(hit) if hit.hit.item_id == relevant_id => 1.0,
        _ => 0.0,
    }
}

#[test]
fn hybrid_recall_beats_single_leg_baselines_on_labeled_fixture() {
    let store = fx::store();
    // Labeled fixture: two queries, one relevant item each. "A" is reachable
    // only through the lexical leg (no vector); "Z" only through the dense leg
    // (no lexical doc). Each single-leg weighting therefore misses exactly the
    // relevant item only the other leg can reach, while hybrid fuses both.
    let lexical_only_relevant = fx::item("A", "bank", "connection reset memory");
    fx::put_lexical(&store, &lexical_only_relevant, "econnreset deploy failure");
    let dense_only_relevant = fx::item("Z", "bank", "alpha overview");
    let query_vector = fx::vector(2);
    fx::put_vector(&store, &dense_only_relevant, query_vector.clone());
    // Vector-only distractor so the lexical-only baseline ranks a non-relevant
    // id first on the "alpha" query (score ties resolve to id order).
    let distractor = fx::item("M", "bank", "general notes");
    fx::put_vector(&store, &distractor, fx::vector(9));

    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: query_vector,
    }));

    let mut hybrid_recall = 0.0;
    let mut dense_only_recall = 0.0;
    let mut lexical_only_recall = 0.0;
    for (query, relevant_id) in [("econnreset", "A"), ("alpha", "Z")] {
        let req = RetrieveRequest::new("bank", query);
        retriever.set_weights(RetrievalWeights {
            dense: 0.5,
            lexical: 0.5,
            importance: 0.0,
        });
        hybrid_recall += recall_at_1(&retriever.retrieve(&req).expect("retrieve"), relevant_id);

        retriever.set_weights(RetrievalWeights {
            dense: 1.0,
            lexical: 0.0,
            importance: 0.0,
        });
        dense_only_recall += recall_at_1(&retriever.retrieve(&req).expect("retrieve"), relevant_id);

        retriever.set_weights(RetrievalWeights {
            dense: 0.0,
            lexical: 1.0,
            importance: 0.0,
        });
        lexical_only_recall +=
            recall_at_1(&retriever.retrieve(&req).expect("retrieve"), relevant_id);
    }

    let queries = 2.0;
    let hybrid = hybrid_recall / queries;
    let dense_only = dense_only_recall / queries;
    let lexical_only = lexical_only_recall / queries;
    assert!(
        hybrid > dense_only && hybrid > lexical_only,
        "hybrid recall@1 {hybrid} must beat dense-only {dense_only} and \
         lexical-only {lexical_only} on the labeled fixture"
    );
    assert_eq!(hybrid, 1.0);
    assert_eq!(dense_only, 0.5);
    assert_eq!(lexical_only, 0.5);
}
