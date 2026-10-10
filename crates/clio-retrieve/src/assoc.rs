// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the association engine core: effective-weight
//! observation (lazy decay + prune-on-observe) and durable co-activation
//! reinforcement of a returned hit set.
//!
//! ## Owns
//! - `AssociationEngine`, `AssocEdgeView`, `GraphNodeView`
//! - The effective-weight observation that prunes decayed coactivation edges
//! - The engine's live co-activation policy cell and its snapshot accessor, so a
//!   caller can retune one `Arc`-shared engine instead of rebuilding it
//!
//! ## Does not own
//! - Tool traversal (`assoc_graph`), storage SQL (`clio-store`), or the weight
//!   formula (`clio_store::assoc`)
//! - The continuous-attribute EMA engine; reinforce never calls it
//!
//! ## Boundary
//!
//! Changes to reinforce/observe should normally require changes here.
//! Changes to traversal tools should not require changes here.

use clio_store::{AssocEdgeRecord, AssocKind, AssocPolicy, Store, effective_weight, should_prune};
use clio_types::{AmError, ErrorCode};
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub use crate::assoc_graph::StoreGraphExpander;

/// Hard cap on nodes visited by one `graph_query` (resource safety).
pub const DEFAULT_QUERY_NODE_CAP: usize = 256;

/// One association edge as seen by an agent tool.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssocEdgeView {
    /// Stable edge id.
    pub edge_id: String,
    /// Stored source endpoint.
    pub source_id: String,
    /// Stored target endpoint.
    pub target_id: String,
    /// The endpoint opposite the queried item.
    pub neighbor_id: String,
    /// `coactivation` or `explicit`.
    pub assoc_kind: AssocKind,
    /// Relationship label (explicit edges only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationship: Option<String>,
    /// Raw stored weight.
    pub weight: f64,
    /// Effective weight after lazy decay (raw for sticky explicit edges).
    pub effective_weight: f64,
    /// Last reinforcement / update stamp.
    pub last_reinforced_at: String,
}

/// One item reached by a bounded `graph_query` traversal.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphNodeView {
    /// Reached item id.
    pub item_id: String,
    /// 1-based hop distance from the seed.
    pub hop: usize,
    /// Edge id used to reach this node.
    pub via_edge_id: String,
    /// Endpoint the traversal came from.
    pub via_source_id: String,
    /// Effective weight of the traversed edge.
    pub effective_weight: f64,
}

/// Association graph engine over one store.
///
/// `Clone` shares the one store handle and the one policy cell: a cloned engine
/// reads and writes the same graph, under the same knobs, rather than a second
/// view of it.
///
/// The co-activation policy sits behind a lock rather than in a plain field
/// because the ranking tools tune it for the process's own config session, and
/// every holder of the engine (the retriever's expansion and reinforcement
/// engine, the retriever's default expander, and the state's graph tools) must
/// see the new value without being rebuilt. The engine stays policy-driven: it
/// never reads configuration itself, and a caller supplies the policy it wants
/// later work to use through [`Self::set_policy`].
#[derive(Clone)]
pub struct AssociationEngine {
    pub(crate) store: Arc<dyn Store>,
    pub(crate) policy: Arc<RwLock<AssocPolicy>>,
    pub(crate) now_millis: Option<i64>,
}

impl std::fmt::Debug for AssociationEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssociationEngine")
            .field("policy", &self.policy())
            .finish_non_exhaustive()
    }
}

impl AssociationEngine {
    /// Build an engine with the given policy and the system clock.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, policy: AssocPolicy) -> Self {
        Self {
            store,
            policy: Arc::new(RwLock::new(policy)),
            now_millis: None,
        }
    }

    /// Build an engine with a fixed clock (deterministic tests).
    #[must_use]
    pub fn with_clock(store: Arc<dyn Store>, policy: AssocPolicy, now_millis: i64) -> Self {
        Self {
            store,
            policy: Arc::new(RwLock::new(policy)),
            now_millis: Some(now_millis),
        }
    }

    /// The current policy knobs, cloned out in one read-lock scope.
    ///
    /// Cloned rather than borrowed so the guard is dropped before the caller
    /// does any work: a concurrent `set_policy` never waits on a scan, and no
    /// read guard is held across storage I/O. A caller that reads several knobs
    /// takes one snapshot and passes it down, so one traversal never mixes two
    /// knob sets.
    #[must_use]
    pub fn policy(&self) -> AssocPolicy {
        self.policy
            .read()
            .expect("assoc policy lock poisoned")
            .clone()
    }

    /// Replace the policy knobs used by reads and writes started after this
    /// call.
    ///
    /// Interior mutability, so a live ranking-environment change reaches the
    /// engine through a shared `Arc` instead of a rebuild.
    pub fn set_policy(&self, policy: AssocPolicy) {
        *self.policy.write().expect("assoc policy lock poisoned") = policy;
    }

    pub(crate) fn now_millis(&self) -> i64 {
        self.now_millis.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        })
    }

    /// Effective weight, pruning a decayed coactivation edge when observed.
    ///
    /// Returns `Ok(None)` when the edge was pruned (coactivation below `w_min`).
    /// `policy` is the caller's one snapshot, so a traversal that observes many
    /// edges reads one knob set rather than re-locking per edge.
    pub(crate) fn observe(
        &self,
        bank: &str,
        edge: &AssocEdgeRecord,
        policy: &AssocPolicy,
    ) -> Result<Option<f64>, AmError> {
        match edge.assoc_kind {
            AssocKind::Explicit => Ok(Some(edge.weight)),
            AssocKind::Coactivation => {
                let eff = effective_weight(
                    edge.weight,
                    &edge.last_updated_at,
                    self.now_millis(),
                    policy.half_life_days,
                );
                if should_prune(eff, policy.w_min) {
                    self.store.delete_assoc_edge(bank, &edge.id)?;
                    self.store.record_assoc_audit(
                        bank,
                        "assoc_prune",
                        Some(&edge.id),
                        "assoc-policy",
                        &serde_json::json!({
                            "edge_id": edge.id,
                            "effective_weight": eff,
                            "w_min": policy.w_min,
                        }),
                    )?;
                    Ok(None)
                } else {
                    Ok(Some(eff))
                }
            }
        }
    }

    /// Durably reinforce every unordered pair of the returned hit set.
    ///
    /// Runs synchronously before the retrieve response; any storage failure
    /// propagates so retrieve fails closed rather than silently skipping.
    ///
    /// # Errors
    /// Empty bank or storage failures.
    pub fn reinforce_hits(&self, bank: &str, hit_ids: &[String]) -> Result<usize, AmError> {
        if bank.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "reinforce requires a bank",
            ));
        }
        let mut ids: Vec<String> = Vec::new();
        let mut seen = BTreeSet::new();
        for id in hit_ids {
            if !id.is_empty() && seen.insert(id.clone()) {
                ids.push(id.clone());
            }
        }
        if ids.len() < 2 {
            return Ok(0);
        }
        let now = self.now_millis();
        let policy = self.policy();
        let mut pairs = 0;
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                self.store
                    .reinforce_assoc_edge(bank, &ids[i], &ids[j], now, policy.eta)?;
                pairs += 1;
            }
        }
        // PII-safe reinforce telemetry (spirit): counts only, no payloads.
        // A failed audit propagates so the handoff is never silently unlogged.
        self.store.record_assoc_audit(
            bank,
            "assoc_reinforce",
            None,
            "assoc-engine",
            &serde_json::json!({ "pairs": pairs, "hits": ids.len() }),
        )?;
        Ok(pairs)
    }
}

#[cfg(test)]
#[path = "assoc_tests.rs"]
mod assoc_tests;

#[cfg(test)]
#[path = "assoc_durable_tests.rs"]
mod assoc_durable_tests;
