// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hybrid retrieval orchestrator: parallel
//! dense + lexical candidate generation, the optional rank-only temporal
//! window arm, domain/temporal filtering, weighted RRF fusion, optional
//! bounded graph expansion, and optional rerank.
//!
//! ## Owns
//! - `HybridRetriever`, leg weighting, the importance shaping step, rerank call
//! - The per-request live ranking snapshot and the candidate fetch/filter
//!   flow feeding fusion and rerank
//! - Resolving the relative-window anchor into a window and adding the arm
//! - The stage at which each requested score floor runs (see `floors`)
//!
//! ## Does not own
//! - Backend search SQL (`clio_store::IndexStore`), embedding (`clio_index`),
//!   the leg fetch/failure calls (`hybrid_legs`), or candidate fetching
//!   (`hybrid_fetch`)
//! - Relative-expression parsing and window resolution (`temporal`)
//! - Request/result DTOs (`types`), fusion math (`fusion`), pack budgets (`compose`)
//! - Final hit mapping, limit, and budget/token accounting (`finalize`)
//! - The floor rules and predicates themselves (`floors`)
//!
//! ## Boundary
//!
//! Changes to retrieval orchestration should normally require changes here.
//! Changes to leg fetch/failure policy (`hybrid_legs`) or fusion math (`fusion`)
//! should not require changes here.
//!
//! # Failure policy
//!
//! Dense coverage may lag the write path: an embed failure or dense-search
//! error degrades to lexical-only with a structured warning instead of failing
//! the query (index-lag contract). Lexical errors (empty bank/query)
//! still fail closed. Rerank errors keep the fused order. A candidate whose
//! content is unreadable (missing or crypto-shredded DEK) is skipped with a
//! warning; one unreadable candidate never aborts the query.
//!
//! # Where a floor runs
//!
//! A requested `semantic` or `keyword` floor prunes its own arm immediately
//! after that arm returns, before items are fetched and before fusion, so the
//! floor is arm-local and never becomes a raw-score cross-arm comparison. A
//! `reranker` floor runs after the rerank stage, and a `final` floor runs in
//! `finalize` on the returned page. With no floor requested every stage is
//! untouched, so a default call is unchanged.
//!
//! # The temporal arm is opt-in and rank-only
//!
//! `query_anchor` must be supplied explicitly and the query must hold exactly
//! one supported relative expression; otherwise the arm contributes nothing and
//! the request fuses exactly the two search legs it always did. The arm returns
//! ids in a deterministic order with no scores, joins fusion as an extra
//! rank-only list, and never uses server time. The dense and lexical legs run
//! first, so a malformed anchor fails closed as `invalid_argument` only after
//! their work is spent — but always before the temporal arm contributes
//! candidates, so the request still aborts.
//!
//! Keep this module focused on retrieval orchestration.

use crate::assoc::{AssociationEngine, StoreGraphExpander};
use crate::dedup::{DedupConfig, DedupedCandidates, dedupe_candidates};
use crate::entity_summary::resolve_entity_controls;
use crate::finalize::{FinalizeArgs, finalize, select_page};
use crate::floors::{apply_keyword_floor, apply_reranker_floor, apply_semantic_floor};
use crate::fusion::{DEFAULT_RRF_K, StageHit};
use crate::hybrid_expand::expand_candidates;
use crate::hybrid_fetch::fetch_items;
use crate::hybrid_legs::{dense_candidates, lexical_candidates, temporal_leg};
use crate::hybrid_rank::{RetentionArgs, apply_rerank, fuse_candidates};
use crate::hybrid_util::{dedupe_ordered, kept_stage, millis, rank_map};
use crate::live_ranking::LiveRanking;
use crate::rerank::Reranker;
use crate::types::{
    DEFAULT_LEG_FETCH, DEFAULT_SHORTLIST_M, GraphExpander, RetrieveMetrics, RetrieveOutcome,
    RetrieveRequest,
};
use clio_config::ranking::{RankingEnv, RetrievalWeights};
use clio_index::Embedder;
use clio_store::{AssocPolicy, IndexStore, Store};
use clio_types::{AmError, ErrorCode, SearchDomain};
use std::sync::Arc;
use std::time::Instant;

/// Hybrid dense + lexical retriever with optional rerank and graph expansion.
pub struct HybridRetriever {
    store: Arc<dyn Store>,
    index: Arc<dyn IndexStore>,
    embedder: Option<Arc<dyn Embedder>>,
    reranker: Option<Arc<dyn Reranker>>,
    expander: Arc<dyn GraphExpander>,
    assoc: Arc<AssociationEngine>,
    reinforce: bool,
    /// The live ranking environment. A request snapshots it once, so a setter
    /// through `&self` lands on the next request.
    live: LiveRanking,
    fusion_k: usize,
    leg_fetch: usize,
    shortlist_m: usize,
}

impl std::fmt::Debug for HybridRetriever {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HybridRetriever")
            .field("fusion_k", &self.fusion_k)
            .field("leg_fetch", &self.leg_fetch)
            .field("shortlist_m", &self.shortlist_m)
            .field("has_embedder", &self.embedder.is_some())
            .field("has_reranker", &self.reranker.is_some())
            .finish_non_exhaustive()
    }
}

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

    /// Build a retriever whose co-activation policy comes from a ranking
    /// environment at construction, and whose leg weights, retention half-life,
    /// and trust ratios are applied live.
    #[must_use]
    pub fn with_ranking_env(
        store: Arc<dyn Store>,
        index: Arc<dyn IndexStore>,
        env: &RankingEnv,
    ) -> Self {
        let mut retriever = Self::new(store, index);
        retriever.set_assoc_policy(AssocPolicy::from_coactivation(&env.coactivation));
        retriever.set_ranking_env(env);
        retriever
    }

    /// Apply the retrieval half of a ranking environment (live leg weights, retention half-life,
    /// and trust ratios) to requests started after this call. The co-activation half is fixed at
    /// construction and cannot be changed on a running retriever; a caller wanting a different
    /// policy builds a new retriever (`with_ranking_env`) or calls `set_assoc_policy` before use.
    pub fn set_ranking_env(&self, env: &RankingEnv) {
        self.live.set_env(env);
    }

    /// Replace the co-activation policy (also retargets the default expander).
    pub fn set_assoc_policy(&mut self, policy: AssocPolicy) {
        self.assoc = Arc::new(AssociationEngine::new(Arc::clone(&self.store), policy));
        self.expander = Arc::new(StoreGraphExpander::new(Arc::clone(&self.assoc)));
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

    /// Run hybrid retrieval. Explicit callers are never gated.
    ///
    /// A linear orchestrator over the stages that live in their own modules
    /// (`hybrid_legs`, `hybrid_expand`, `hybrid_fetch`, `hybrid_rank`,
    /// `floors`, `dedup`, `finalize`), so its length is the documented stage
    /// order rather than undecomposed logic.
    ///
    /// # Errors
    /// Missing bank, empty query, zero limit, invalid domains, or lexical failures.
    #[allow(clippy::too_many_lines)]
    pub fn retrieve(&self, req: &RetrieveRequest) -> Result<RetrieveOutcome, AmError> {
        Self::validate(req)?;
        // One read-lock scope: copy the live knobs this request reads and drop
        // the guard before any work. A setter thus lands on the next request.
        let snapshot = self.live.snapshot();
        let started = Instant::now();
        let domains = SearchDomain::parse_all(&req.domains)?;
        let mut warnings = Vec::new();
        let dense_started = Instant::now();
        let dense_hits = self.dense_leg(req, &mut warnings);
        let dense_ms = millis(dense_started);
        let lexical_started = Instant::now();
        let lexical_hits = self.lexical_leg(req)?;
        let lexical_ms = millis(lexical_started);
        let (window, temporal_ids) = temporal_leg(self.store.as_ref(), req, self.leg_fetch)?;
        let mut candidate_ids = dedupe_ordered(
            dense_hits
                .iter()
                .map(|h| &h.id)
                .chain(lexical_hits.iter().map(|h| &h.id))
                .chain(temporal_ids.iter()),
        );
        let expand_started = Instant::now();
        let (expanded, graph_new) = expand_candidates(
            self.expander.as_ref(),
            self.assoc.policy().max_hops as usize,
            self.shortlist_m,
            req,
            &candidate_ids,
        )?;
        let expand_ms = millis(expand_started);
        candidate_ids.extend(graph_new.iter().cloned());
        let items = fetch_items(
            self.store.as_ref(),
            &req.bank,
            &candidate_ids,
            &domains,
            req,
            &mut warnings,
        )?;
        let dense_filtered = kept_stage(&dense_hits, &items);
        let lexical_filtered = kept_stage(&lexical_hits, &items);
        let dense_rank = rank_map(&dense_filtered);
        let lexical_rank = rank_map(&lexical_filtered);
        let (dense_candidates, lexical_candidates) = (dense_filtered.len(), lexical_filtered.len());
        // The temporal arm is rank-only: keep the ids that survived the shared
        // fetch, in the arm's own order. That order is the arm's fusion list,
        // and each hit's reported temporal rank is its 1-based position in it,
        // carried as the arm's own fusion provenance rather than a separate
        // map that could drift from the list.
        let temporal_kept: Vec<String> = temporal_ids
            .iter()
            .filter(|id| items.contains_key(*id))
            .cloned()
            .collect();
        let temporal_candidates = window.as_ref().map(|_| temporal_kept.len());
        // The curve's clock: the request's fixed `now` when one was supplied,
        // else the system clock, so only the system default is time-dependent.
        let now = req
            .now
            .clone()
            .unwrap_or_else(crate::filter::system_now_iso);
        let fused = fuse_candidates(
            &snapshot.weights,
            self.fusion_k,
            &dense_filtered,
            &lexical_filtered,
            &temporal_kept,
            &items,
            &expanded,
            RetentionArgs {
                active: req.retention_curve,
                floor: req.retention_floor,
                half_life_days: snapshot.half_life_days,
                now: &now,
            },
            snapshot.trust,
        );
        // Genuinely graph-expanded: fused ids that neither search leg produced,
        // so ids already fetched as dense/lexical candidates are not counted.
        let graph_expanded = fused.iter().filter(|h| graph_new.contains(&h.id)).count();
        let rerank_started = Instant::now();
        let (fused, reranked) = apply_rerank(
            self.reranker.as_ref(),
            self.shortlist_m,
            &req.query,
            &items,
            fused,
            &mut warnings,
        );
        let rerank_ms = millis(rerank_started);
        // A `reranker` floor runs after the rerank stage, where a score exists.
        // Reinforcement observes the pre-dedup fused page under the same
        // limit and budget, so suppression never silently drops reinforcement.
        // It is a co-activation side effect, not part of the returned page, and
        // it is not floor-independent: the arm floors already pruned this list,
        // and the `reranker` floor prunes it here, so the reinforced page is the
        // candidate page of the floored query. `final` runs later, in
        // `finalize`, and leaves this page alone.
        let fused = apply_reranker_floor(fused, req.min_scores.reranker, |hit| hit.rerank_score);
        let reinforce_ids = select_page(&fused, &items, req.limit, req.budget_tokens);
        let deduped = dedupe_candidates(
            fused,
            &items,
            DedupConfig {
                scope: req.recall_scope,
                prefer_consolidated: req.prefer_consolidated,
                tolerance: req.duplicate_tolerance_read,
            },
            req.limit,
            req.budget_tokens,
        );
        let backfill_baseline =
            select_page(&deduped.scope_page, &items, req.limit, req.budget_tokens);
        let DedupedCandidates {
            hits: deduped_hits,
            meta: dedup_meta,
            ..
        } = deduped;
        let mut outcome = finalize(FinalizeArgs {
            req,
            items: &items,
            dense_rank: &dense_rank,
            lexical_rank: &lexical_rank,
            fused: deduped_hits,
            dense_candidates,
            lexical_candidates,
            temporal_candidates,
            reranked,
            graph_expanded,
            metrics: RetrieveMetrics {
                dense_ms,
                lexical_ms,
                expand_ms,
                rerank_ms,
                total_ms: millis(started),
            },
            warnings,
            dedup: dedup_meta,
            backfill_baseline,
        });
        if self.reinforce {
            // Co-activation growth MUST be durably handed off before the
            // response; a failed handoff fails closed. Validation errors pass through.
            outcome.pairs_reinforced = self.reinforce(&req.bank, &reinforce_ids)?;
        }
        Ok(outcome)
    }

    /// Durable handoff of one reinforce set; validation errors pass through.
    fn reinforce(&self, bank: &str, ids: &[String]) -> Result<usize, AmError> {
        self.assoc
            .reinforce_hits(bank, ids)
            .map_err(|e| match e.code {
                ErrorCode::MissingBank | ErrorCode::InvalidArgument | ErrorCode::OutOfRange => e,
                _ => AmError::new(
                    ErrorCode::CoactivationHandoffFailed,
                    format!("co-activation durability handoff failed: {}", e.message),
                ),
            })
    }
    /// Reject a request whose own parameters cannot be honored: a missing bank
    /// or query, a zero limit, a nonsensical score floor, or a contradictory
    /// pair of entity controls. Validated here so every entry point, including
    /// a direct in-process caller that never went through a binding, fails
    /// closed the same way.
    fn validate(req: &RetrieveRequest) -> Result<(), AmError> {
        if req.bank.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "retrieve requires a non-empty bank",
            ));
        }
        if req.query.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "retrieve requires a non-empty query",
            ));
        }
        if req.limit == 0 {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "retrieve limit must be positive",
            ));
        }
        req.min_scores.validate()?;
        resolve_entity_controls(req.include_entities, req.entity_summary)?;
        Ok(())
    }

    /// Dense candidates with their raw cosine distance, minus any `semantic`
    /// floor (warning-degrading).
    ///
    /// The floor is applied here, to this arm's own list and on the same
    /// similarity scale the `scores.semantic` field publishes, so it prunes
    /// before items are fetched and before fusion and never becomes a
    /// cross-arm score comparison.
    fn dense_leg(&self, req: &RetrieveRequest, warnings: &mut Vec<String>) -> Vec<StageHit> {
        apply_semantic_floor(
            dense_candidates(
                self.embedder.as_ref(),
                self.index.as_ref(),
                req,
                self.leg_fetch,
                warnings,
            ),
            req.min_scores.semantic,
        )
    }

    /// Lexical candidates with their raw backend score, minus any `keyword`
    /// floor (fail-closed).
    ///
    /// The floor is applied to this arm's own list on the same raw backend
    /// scale the `scores.keyword` field publishes, so the other arm's
    /// candidates are untouched.
    fn lexical_leg(&self, req: &RetrieveRequest) -> Result<Vec<StageHit>, AmError> {
        Ok(apply_keyword_floor(
            lexical_candidates(self.index.as_ref(), req, self.leg_fetch)?,
            req.min_scores.keyword,
        ))
    }
}

#[cfg(test)]
#[path = "hybrid_tests.rs"]
mod hybrid_tests;

#[cfg(test)]
#[path = "hybrid_quality_tests.rs"]
mod hybrid_quality_tests;

#[cfg(test)]
#[path = "hybrid_rerank_tests.rs"]
mod hybrid_rerank_tests;

#[cfg(test)]
#[path = "hybrid_unreadable_tests.rs"]
mod hybrid_unreadable_tests;

#[cfg(test)]
#[path = "hybrid_pg_parity_tests.rs"]
mod hybrid_pg_parity_tests;
