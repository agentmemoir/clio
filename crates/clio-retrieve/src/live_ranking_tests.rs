// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live ranking store tests: the
//! system-default start, whole-environment and weights-only replacement, the
//! snapshot being a copy rather than a live view, and the retriever-level
//! guarantee that a `set_ranking_env` call through a shared handle governs the
//! next retrieval.
//!
//! ## Owns
//! - Pure `LiveRanking` assertions with no clock or store
//! - The end-to-end liveness assertion that the `&self` setter lands on the
//!   next read of one shared retriever
//!
//! ## Does not own
//! - The knobs' defaults and validation (`clio-config`)
//! - How each retrieval stage reads a snapshot (`hybrid`, `hybrid_rank`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::fixtures_tests as fx;
use crate::types::RetrieveRequest;
use clio_config::ranking::{
    DEFAULT_RETENTION_HALF_LIFE_DAYS, RankingEnv, RetrievalWeights, TrustRatios,
};
use std::sync::Arc;

#[test]
fn new_starts_from_the_system_defaults() {
    let snapshot = LiveRanking::new().snapshot();
    assert_eq!(snapshot.weights, RetrievalWeights::default());
    assert_eq!(
        snapshot.half_life_days.to_bits(),
        DEFAULT_RETENTION_HALF_LIFE_DAYS.to_bits()
    );
    assert_eq!(snapshot.trust, TrustRatios::default());
}

#[test]
fn set_env_replaces_every_knob_at_once() {
    let live = LiveRanking::new();
    let mut env = RankingEnv::system_defaults();
    env.retrieval = RetrievalWeights {
        dense: 0.6,
        lexical: 0.3,
        importance: 0.1,
    };
    env.retention.half_life_days = 7.0;
    env.trust = TrustRatios {
        user_stated: 0.9,
        third_party: 0.8,
        agent_inferred: 0.7,
    };
    live.set_env(&env);
    let snapshot = live.snapshot();
    assert_eq!(snapshot.weights, env.retrieval);
    assert_eq!(snapshot.half_life_days.to_bits(), 7.0f64.to_bits());
    assert_eq!(snapshot.trust, env.trust);
}

#[test]
fn set_weights_leaves_the_other_knobs_alone() {
    let live = LiveRanking::new();
    let weights = RetrievalWeights {
        dense: 0.5,
        lexical: 0.25,
        importance: 0.25,
    };
    live.set_weights(weights.clone());
    assert_eq!(live.weights(), weights);
    let snapshot = live.snapshot();
    assert_eq!(
        snapshot.half_life_days.to_bits(),
        DEFAULT_RETENTION_HALF_LIFE_DAYS.to_bits()
    );
    assert_eq!(snapshot.trust, TrustRatios::default());
}

#[test]
fn a_snapshot_is_a_copy_not_a_live_view() {
    let live = LiveRanking::new();
    let before = live.snapshot();
    let mut env = RankingEnv::system_defaults();
    env.trust.agent_inferred = 0.5;
    live.set_env(&env);
    assert_eq!(before.trust, TrustRatios::default());
    assert_eq!(
        live.snapshot().trust.agent_inferred.to_bits(),
        0.5f64.to_bits()
    );
}

#[test]
fn a_live_env_set_through_a_shared_retriever_governs_the_next_read() {
    let store = fx::store();
    let entry = fx::belief_item("A", "bank", "alpha memory");
    fx::put_lexical(&store, &entry, "alpha memory token");
    let shared = Arc::new(fx::retriever(&store));
    let mut req = RetrieveRequest::new("bank", "alpha");
    req.now = Some("2026-01-02T00:00:00Z".into());

    let mut neutral = RankingEnv::system_defaults();
    neutral.trust = TrustRatios {
        user_stated: 1.0,
        third_party: 1.0,
        agent_inferred: 1.0,
    };
    shared.set_ranking_env(&neutral);
    let base = shared.retrieve(&req).expect("retrieve");
    let base_hit = base
        .hits
        .iter()
        .find(|h| h.hit.item_id == "A")
        .expect("A hit");
    assert_eq!(base_hit.scores.trust_factor.to_bits(), 1.0f64.to_bits());

    // The `&self` setter through the shared handle must govern the next read.
    let mut demoted = RankingEnv::system_defaults();
    demoted.trust = TrustRatios {
        user_stated: 1.0,
        third_party: 1.0,
        agent_inferred: 0.5,
    };
    shared.set_ranking_env(&demoted);
    let next = shared.retrieve(&req).expect("retrieve");
    let next_hit = next
        .hits
        .iter()
        .find(|h| h.hit.item_id == "A")
        .expect("A hit");
    assert_eq!(next_hit.scores.trust_factor.to_bits(), 0.5f64.to_bits());
    assert_eq!(
        next_hit.score.to_bits(),
        (base_hit.score * next_hit.scores.trust_factor).to_bits()
    );
}
