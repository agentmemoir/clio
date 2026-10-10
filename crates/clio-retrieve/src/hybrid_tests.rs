// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for hybrid-retrieval behavior tests: gate
//! override, lexical/dense leg coverage, domain/temporal filters, bank
//! isolation, graph expansion, determinism, and weights.
//!
//! ## Owns
//! - End-to-end retriever assertions, including failure-degradation warnings
//!
//! ## Does not own
//! - Rerank ordering/fallback behavior (`hybrid_rerank_tests`), fusion-math
//!   fixtures (`fusion_tests`), budget trimming (`finalize_tests`),
//!   retrieval-quality baselines (`hybrid_quality_tests`), or gate internals
//!   (`intent_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the hybrid orchestrator contract.

use crate::fixtures_tests as fx;
use crate::hybrid::HybridRetriever;
use crate::intent::{GateDecision, intent_gate};
use crate::types::{
    DEFAULT_LEG_FETCH, DEFAULT_LIMIT, DEFAULT_SHORTLIST_M, GraphExpander, NoopGraphExpander,
    RetrieveRequest,
};
use clio_config::ranking::IntentGateThresholds;
use clio_config::ranking::RankingEnv;
use clio_config::ranking::RetrievalWeights;
use clio_store::Store;
use clio_types::TimeAxis;
use std::sync::Arc;

fn request(bank: &str, query: &str) -> RetrieveRequest {
    RetrieveRequest::new(bank, query)
}

fn ids(out: &crate::types::RetrieveOutcome) -> Vec<String> {
    out.hits.iter().map(|h| h.hit.item_id.clone()).collect()
}

#[test]
fn new_request_defaults() {
    let req = RetrieveRequest::new("bank", "query");
    assert_eq!(req.limit, DEFAULT_LIMIT);
    assert!(req.domains.is_empty(), "{:?}", req.domains);
    assert_eq!(req.time_axis, TimeAxis::Valid);
    assert!(!req.expand_graph);
    assert!(req.as_of.is_none());
    assert!(req.budget_tokens.is_none());
    // The optional controls are off by default: no floor, entities published,
    // no summary.
    assert!(req.min_scores.is_empty(), "no score floor by default");
    assert!(
        req.include_entities,
        "entity names are published by default"
    );
    assert!(!req.entity_summary, "no summary unless it is requested");
}

#[test]
fn t06_hybrid_includes_lexical_only_exact_hit() {
    let store = fx::store();
    let dense_item = fx::item("A", "bank", "alpha overview");
    let lexical_item = fx::item("B", "bank", "ECONNRESET deploy failure");
    let query_vector = fx::vector(1);
    fx::put_vector(&store, &dense_item, query_vector.clone());
    fx::put_lexical(&store, &lexical_item, "ECONNRESET exact");

    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: query_vector,
    }));
    retriever.set_fusion(60, 1, 60);

    let out = retriever
        .retrieve(&request("bank", "ECONNRESET"))
        .expect("retrieve");
    assert!(ids(&out).contains(&"B".to_owned()), "hits: {:?}", ids(&out));
    let hit_b = out.hits.iter().find(|h| h.hit.item_id == "B").expect("B");
    assert!(hit_b.lexical_rank.is_some());
    assert!(out.dense_candidates <= 1);
    assert!(out.lexical_candidates >= 1);
    assert!(out.metrics.total_ms < 60_000);
}

#[test]
fn unreadable_candidate_is_skipped_with_a_warning() {
    // A candidate whose DEK is gone (or crypto-shredded) must not abort the
    // query: it is skipped with a warning and the readable hits still return.
    let (store, kms) = fx::store_and_kms();
    let mut readable = fx::item("A", "bank", "alpha readable");
    readable.subject_id = "subj-a".into();
    let mut shredded = fx::item("B", "bank", "alpha shredded");
    shredded.subject_id = "subj-b".into();
    fx::put_lexical(&store, &readable, "alpha");
    fx::put_lexical(&store, &shredded, "alpha");
    kms.destroy_dek("subj-b").expect("destroy DEK");

    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request("bank", "alpha"))
        .expect("retrieve");
    assert!(ids(&out).contains(&"A".to_owned()), "hits: {:?}", ids(&out));
    assert!(
        !ids(&out).contains(&"B".to_owned()),
        "hits: {:?}",
        ids(&out)
    );
    assert!(
        out.warnings.iter().any(|w| w.contains("unreadable")),
        "warnings: {:?}",
        out.warnings
    );
}

#[test]
fn t09_as_of_and_time_axis_filter_candidates() {
    let store = fx::store();
    let mut expired = fx::item("E", "bank", "expired fact");
    expired.valid_from = "2026-01-01".into();
    expired.valid_until = Some("2026-03-01".into());
    fx::put_lexical(&store, &expired, "expired fact token");
    let mut tx_only = fx::item("T", "bank", "tx fact");
    tx_only.tx_from = "2026-02-01".into();
    fx::put_lexical(&store, &tx_only, "tx fact token");

    let retriever = fx::retriever(&store);
    let mut req = request("bank", "fact");
    req.as_of = Some("2026-04-01".into());
    let out = retriever.retrieve(&req).expect("retrieve");
    assert!(!ids(&out).contains(&"E".to_owned()));

    req.as_of = Some("2026-02-01".into());
    let out = retriever.retrieve(&req).expect("retrieve");
    assert!(ids(&out).contains(&"E".to_owned()));

    req.as_of = Some("2026-01-15".into());
    req.time_axis = TimeAxis::Transaction;
    let out = retriever.retrieve(&req).expect("retrieve");
    assert!(!ids(&out).contains(&"T".to_owned()));
}

#[test]
fn t12_domain_filter_returns_only_requested_domains() {
    let store = fx::store();
    let semantic = fx::semantic_item("S", "bank", "config schema reference");
    let episodic = fx::item("E", "bank", "config schema run");
    fx::put_lexical(&store, &semantic, "config schema");
    fx::put_lexical(&store, &episodic, "config schema");

    let retriever = fx::retriever(&store);
    let mut req = request("bank", "config");
    req.domains = vec!["episodic".into()];
    let out = retriever.retrieve(&req).expect("retrieve");
    assert_eq!(ids(&out), vec!["E".to_owned()]);

    req.domains = vec!["semantic".into()];
    let out = retriever.retrieve(&req).expect("retrieve");
    assert_eq!(ids(&out), vec!["S".to_owned()]);
}

#[test]
fn t13_bank_isolation_returns_no_cross_bank_hits() {
    let store = fx::store();
    let entry = fx::item("A", "bank-a", "secret project");
    fx::put_lexical(&store, &entry, "secret project");
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request("bank-b", "secret"))
        .expect("retrieve");
    assert!(out.hits.is_empty(), "{:?}", out.hits);
}

#[test]
fn t04_explicit_retrieve_runs_after_gate_skip() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "what city do I live in");
    fx::put_lexical(&store, &entry, "city live");
    let decision: GateDecision = intent_gate("hi", &IntentGateThresholds::default());
    assert!(!decision.retrieval_needed);
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request("bank", "city"))
        .expect("retrieve");
    assert_eq!(ids(&out), vec!["A".to_owned()]);
}

#[test]
fn dense_outage_degrades_to_lexical_only() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "alpha");
    fx::put_lexical(&store, &entry, "alpha");
    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FailingEmbedder));
    let out = retriever
        .retrieve(&request("bank", "alpha"))
        .expect("retrieve");
    assert_eq!(ids(&out), vec!["A".to_owned()]);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("dense leg unavailable"))
    );
}

#[test]
fn dense_dim_mismatch_degrades_to_lexical_only() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "alpha");
    fx::put_lexical(&store, &entry, "alpha");
    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: vec![0.0; 8],
    }));
    let out = retriever
        .retrieve(&request("bank", "alpha"))
        .expect("retrieve");
    assert_eq!(ids(&out), vec!["A".to_owned()]);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("dense leg unavailable"))
    );
}

#[test]
fn empty_embed_batch_is_ignored() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "alpha");
    fx::put_lexical(&store, &entry, "alpha");
    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::EmptyEmbedder));
    let out = retriever
        .retrieve(&request("bank", "alpha"))
        .expect("retrieve");
    assert_eq!(ids(&out), vec!["A".to_owned()]);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

#[test]
fn graph_expansion_adds_seed_neighbors_when_enabled() {
    let store = fx::store();
    let seed = fx::item("A", "bank", "seed token");
    fx::put_lexical(&store, &seed, "seed token");
    let neighbor = fx::item("C", "bank", "neighbor");
    store
        .create_memory_item(&neighbor, "agent")
        .expect("neighbor");

    let mut retriever = fx::retriever(&store);
    retriever.set_expander(Arc::new(fx::StaticExpander {
        // "A" is already a lexical candidate; it must not count as
        // graph-expanded, only the genuinely new neighbor "C" does.
        ids: vec!["C".into(), "A".into()],
    }));
    let mut req = request("bank", "seed");
    req.expand_graph = true;
    let out = retriever.retrieve(&req).expect("retrieve");
    assert!(ids(&out).contains(&"C".to_owned()));
    assert_eq!(out.graph_expanded, 1);

    let out_plain = retriever
        .retrieve(&request("bank", "seed"))
        .expect("retrieve");
    assert!(!ids(&out_plain).contains(&"C".to_owned()));
}

#[test]
fn graph_expander_failure_propagates() {
    let store = fx::store();
    let seed = fx::item("A", "bank", "seed token");
    fx::put_lexical(&store, &seed, "seed token");
    let mut retriever = fx::retriever(&store);
    retriever.set_expander(Arc::new(fx::FailExpander));
    let mut req = request("bank", "seed");
    req.expand_graph = true;
    assert!(retriever.retrieve(&req).is_err());
}

#[test]
fn noop_expander_returns_nothing() {
    let expander = NoopGraphExpander;
    let edges = expander.expand("bank", &["A".into()], 1).expect("expand");
    assert!(edges.is_empty(), "{edges:?}");
}

#[test]
fn validate_rejects_bad_requests() {
    let store = fx::store();
    let retriever = fx::retriever(&store);
    assert!(
        retriever
            .retrieve(&RetrieveRequest::new("", "query"))
            .is_err()
    );
    assert!(
        retriever
            .retrieve(&RetrieveRequest::new("bank", "   "))
            .is_err()
    );
    let mut zero = RetrieveRequest::new("bank", "query");
    zero.limit = 0;
    assert!(retriever.retrieve(&zero).is_err());
    let mut bad_domain = RetrieveRequest::new("bank", "query");
    bad_domain.domains = vec!["bogus".into()];
    assert!(retriever.retrieve(&bad_domain).is_err());
}

#[test]
fn t14_weight_change_changes_order_deterministically() {
    let store = fx::store();
    let dense_item = fx::item("D", "bank", "dense only");
    let lexical_item = fx::item("L", "bank", "lexical only");
    let query_vector = fx::vector(2);
    fx::put_vector(&store, &dense_item, query_vector.clone());
    fx::put_lexical(&store, &lexical_item, "lexical");

    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: query_vector,
    }));
    retriever.set_fusion(60, 1, 60);
    retriever.set_weights(RetrievalWeights {
        dense: 1.0,
        lexical: 0.0,
        importance: 0.0,
    });
    assert_eq!(retriever.weights().dense, 1.0);
    let dense_first = ids(&retriever
        .retrieve(&request("bank", "lexical"))
        .expect("retrieve"));
    assert_eq!(dense_first.first().map(String::as_str), Some("D"));

    retriever.set_weights(RetrievalWeights {
        dense: 0.0,
        lexical: 1.0,
        importance: 0.0,
    });
    let lexical_first = ids(&retriever
        .retrieve(&request("bank", "lexical"))
        .expect("retrieve"));
    assert_eq!(lexical_first.first().map(String::as_str), Some("L"));
    assert_ne!(dense_first, lexical_first);
}

#[test]
fn repeated_retrieve_is_deterministic() {
    let store = fx::store();
    for id in ["A", "B", "C"] {
        let entry = fx::item(id, "bank", "shared token");
        fx::put_lexical(&store, &entry, "shared token");
    }
    let retriever = fx::retriever(&store);
    let first = ids(&retriever
        .retrieve(&request("bank", "shared"))
        .expect("retrieve"));
    let second = ids(&retriever
        .retrieve(&request("bank", "shared"))
        .expect("retrieve"));
    assert_eq!(first, second);
}

#[test]
fn debug_format_and_constants() {
    let store = fx::store();
    let retriever = fx::retriever(&store);
    let debug = format!("{retriever:?}");
    assert!(debug.contains("HybridRetriever"));
    assert_eq!((DEFAULT_LEG_FETCH, DEFAULT_SHORTLIST_M), (60, 60));
}

#[test]
fn snapshot_ref_present_for_snapshot_items() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "shared token");
    fx::put_lexical(&store, &entry, "shared token");
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request("bank", "shared"))
        .expect("retrieve");
    assert_eq!(out.hits[0].snapshot_ref.as_deref(), Some("A"));
}

#[test]
fn t15_env_half_life_shapes_the_aged_hit_and_its_rank() {
    // Fixed clock: the aged item leads before the curve, so a long half-life
    // keeps that lead and a short one does not.
    const NOW: &str = "2027-01-01T00:00:00Z";
    let store = fx::store();
    let mut aged = fx::item("aged", "bank", "aged retention token");
    aged.created_at = "2026-01-01T00:00:00Z".into();
    aged.admission_score = Some(1.0);
    let mut fresh = fx::item("fresh", "bank", "fresh retention token");
    fresh.created_at = NOW.into();
    fresh.admission_score = Some(0.0);
    fx::put_lexical(&store, &aged, "aged retention token");
    fx::put_lexical(&store, &fresh, "fresh retention token");

    let mut req = request("bank", "retention");
    req.now = Some(NOW.into());
    req.retention_curve = true;
    req.retention_floor = 0.25;

    let retrieve_at = |half_life_days: f64| {
        let retriever = fx::retriever(&store);
        let mut env = RankingEnv::system_defaults();
        // Identical in both runs; only the half-life differs.
        env.retrieval.importance = 1.0;
        env.retention.half_life_days = half_life_days;
        retriever.set_ranking_env(&env);
        retriever.retrieve(&req).expect("retrieve")
    };
    let (short, long) = (retrieve_at(1.0), retrieve_at(365.0));

    let final_score = |out: &crate::types::RetrieveOutcome, id: &str| {
        out.hits
            .iter()
            .find(|h| h.hit.item_id == id)
            .expect("hit")
            .score
    };
    assert!(
        final_score(&short, "aged") < final_score(&long, "aged"),
        "the 1-day half-life must decay the aged hit more"
    );
    let (fresh_short, fresh_long) = (final_score(&short, "fresh"), final_score(&long, "fresh"));
    assert_eq!(fresh_short.to_bits(), fresh_long.to_bits());
    assert_eq!(ids(&short).first().map(String::as_str), Some("fresh"));
    assert_eq!(ids(&long).first().map(String::as_str), Some("aged"));
}

#[test]
fn retriever_can_be_built_with_trait_objects() {
    let store = fx::store();
    let retriever = HybridRetriever::new(
        Arc::clone(&store) as Arc<dyn clio_store::Store>,
        Arc::clone(&store) as Arc<dyn clio_store::IndexStore>,
    );
    let expander: Arc<dyn GraphExpander> = Arc::new(NoopGraphExpander);
    let _ = expander;
    assert!(retriever.weights().dense > 0.0);
}
