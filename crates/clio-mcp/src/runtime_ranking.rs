// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for handing the process's *current* ranking
//! environment to the engines that read it, so a `ranking_env_set` in this
//! process governs work started afterwards: the hub engine receives the derived
//! co-activation policy, and the shared retriever receives the retrieval half
//! (leg weights, retention half-life, and trust ratios).
//!
//! ## Owns
//! - `McpState::ranking_policy` (the derived `AssocPolicy` for a given env)
//! - `McpState::apply_ranking_policy` (push the live value into the engines)
//!
//! ## Does not own
//! - Ranking weight validation and the env type itself (`clio-config` owns those)
//! - The `AssocPolicy` conversion, which stays in `clio-store`
//! - When a wave runs and which waves exist (see `mutator_tools`)
//!
//! ## Boundary
//!
//! Changes to how a ranking-environment change reaches an engine should normally
//! require changes here. Changes to ranking validation or admission math should
//! not require changes here.
//!
//! Keep this module focused on live ranking to engine policy plumbing.

use crate::runtime::McpState;
use clio_config::ranking::RankingEnv;
use clio_store::AssocPolicy;

impl McpState {
    /// The co-activation policy described by `env`.
    ///
    /// The one conversion, so the hub path and the retriever can never derive
    /// two different policies from the same environment.
    #[must_use]
    pub fn ranking_policy(env: &RankingEnv) -> AssocPolicy {
        AssocPolicy::from_coactivation(&env.coactivation)
    }

    /// Push the process's current ranking environment into the engines that hold
    /// a co-activation policy and into the retriever's live ranking snapshot.
    ///
    /// The ranking tools mutate the config runtime's in-memory `RankingEnv`
    /// (nothing persists it, and a fresh process starts from the effective
    /// defaults), so the value that governs a wave or a retrieval is read at
    /// call time rather than captured when the engines were built. A wave or a
    /// retrieval already in flight keeps the environment it started with.
    pub fn apply_ranking_policy(&self) {
        let env = {
            let rt = self.config.lock().expect("config runtime mutex poisoned");
            rt.ranking_env().clone()
        };
        if let Some(hub) = self.hub.as_ref() {
            hub.set_policy(Self::ranking_policy(&env));
        }
        // The retriever takes one snapshot per request, so the retrieval
        // weights, retention half-life, and trust ratios set here govern the
        // next retrieval. Its own co-activation engine stays the value it was
        // built with; the hub path above carries the live co-activation policy.
        self.retriever.set_ranking_env(&env);
    }
}

#[cfg(test)]
#[path = "runtime_ranking_tests.rs"]
mod runtime_ranking_tests;
