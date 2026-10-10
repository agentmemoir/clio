// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for constructing a `HybridRetriever` and retuning
//! one after construction: the two constructors, the live ranking-environment
//! push, and the setter surface for the co-activation policy, reinforcement,
//! embedder, reranker, expander, retrieval weights, and fusion sizes.
//!
//! ## Owns
//! - `HybridRetriever::{new, with_ranking_env}` and the tuning setters
//! - The interior-lock push that retunes the retriever without a rebuild
//!
//! ## Does not own
//! - The retrieval orchestrator and its stages (`hybrid`)
//! - The live ranking cell and its snapshot (`live_ranking`)
//! - The association engine and its own policy cell (`assoc`)
//!
//! ## Boundary
//!
//! Changes to how a retriever is built or retuned should normally require
//! changes here. Changes to the retrieval stages should not require changes
//! here.
//!
//! Keep this module focused on retriever construction and tuning.

use super::HybridRetriever;
use crate::assoc::{AssociationEngine, StoreGraphExpander};
use crate::fusion::DEFAULT_RRF_K;
use crate::live_ranking::LiveRanking;
use crate::rerank::Reranker;
use crate::types::{DEFAULT_LEG_FETCH, DEFAULT_SHORTLIST_M, GraphExpander};
use clio_config::ranking::{RankingEnv, RetrievalWeights};
use clio_index::Embedder;
use clio_store::{AssocPolicy, IndexStore, Store};
use std::sync::Arc;

impl HybridRetriever {
    /// Build a retriever with system-default weights and no embedder/reranker.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, index: Arc<dyn IndexStore>) -> Self {
        let assoc = Arc::new(AssociationEngine::new(
            Arc::clone(&store),
            AssocPolicy::default(),
        ));
        Self {
            store,
            index,
            embedder: None,
            reranker: None,
            expander: Arc::new(StoreGraphExpander::new(Arc::clone(&assoc))),
            assoc,
            reinforce: true,
            live: LiveRanking::new(),
            fusion_k: DEFAULT_RRF_K,
            leg_fetch: DEFAULT_LEG_FETCH,
            shortlist_m: DEFAULT_SHORTLIST_M,
        }
    }

    /// Build a retriever whose co-activation policy and live ranking knobs both
    /// come from a ranking environment.
    #[must_use]
    pub fn with_ranking_env(
        store: Arc<dyn Store>,
        index: Arc<dyn IndexStore>,
        env: &RankingEnv,
    ) -> Self {
        let retriever = Self::new(store, index);
        retriever.set_ranking_env(env);
        retriever
    }

    /// Apply a ranking environment to requests started after this call: the
    /// retrieval half (leg weights, retention half-life, trust ratios) and the
    /// co-activation half (the association policy that graph expansion and
    /// reinforcement read). Both halves land on the engines the hub path
    /// already retunes, so one knob has one behavior on every path.
    pub fn set_ranking_env(&self, env: &RankingEnv) {
        self.live.set_env(env);
        self.assoc
            .set_policy(AssocPolicy::from_coactivation(&env.coactivation));
    }

    /// Replace the co-activation policy used by requests started after this
    /// call. The default expander reads the engine's shared policy cell, so it
    /// needs no retarget.
    pub fn set_assoc_policy(&self, policy: AssocPolicy) {
        self.assoc.set_policy(policy);
    }

    /// Enable/disable the co-activation reinforce hook (rollback switch).
    pub fn set_reinforcement(&mut self, enabled: bool) {
        self.reinforce = enabled;
    }

    /// Attach a dense embedder (enables the dense leg).
    pub fn set_embedder(&mut self, embedder: Arc<dyn Embedder>) {
        self.embedder = Some(embedder);
    }

    /// Attach a reranker (enables the optional second stage).
    pub fn set_reranker(&mut self, reranker: Arc<dyn Reranker>) {
        self.reranker = Some(reranker);
    }

    /// Disable rerank.
    pub fn clear_reranker(&mut self) {
        self.reranker = None;
    }

    /// Attach a graph expander.
    pub fn set_expander(&mut self, expander: Arc<dyn GraphExpander>) {
        self.expander = expander;
    }

    /// Set retrieval weights through the interior lock. Only the weights half
    /// of the ranking environment; the retention half-life and trust ratios
    /// are reachable through `set_ranking_env`.
    pub fn set_weights(&self, weights: RetrievalWeights) {
        self.live.set_weights(weights);
    }

    /// Override fusion/shortlist sizes.
    pub fn set_fusion(&mut self, k: usize, leg_fetch: usize, shortlist_m: usize) {
        self.fusion_k = k;
        self.leg_fetch = leg_fetch;
        self.shortlist_m = shortlist_m;
    }

    /// Current retrieval weights (a snapshot clone of the live value).
    #[must_use]
    pub fn weights(&self) -> RetrievalWeights {
        self.live.weights()
    }
}
