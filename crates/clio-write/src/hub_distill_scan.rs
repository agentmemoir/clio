// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two read-only scans a `hub_distill` wave
//! runs against one co-activation policy: which nodes are over-degree hubs, and
//! which neighbours one hub has.
//!
//! ## Owns
//! - `HubDistillEngine::effective`, the explicit-versus-decayed weight rule
//! - `HubDistillEngine::eligible_hubs_under`, the over-degree hub scan
//! - `HubDistillEngine::top_neighbors`, the ranked neighbour cut for one hub
//!
//! ## Does not own
//! - Wave scheduling, eligibility, and the distill pipeline (`hub_distill`)
//! - The policy cell itself, and `set_policy` (`hub_distill`)
//! - Report and item shapes and the pure builders (`hub_distill_model`)
//!
//! ## Boundary
//!
//! Split out of `hub_distill.rs` to keep that file inside the 450-line limit.
//! Changes to how a weight is computed or how neighbors are ranked belong here;
//! changes to when a wave runs or what it does with a scan belong in
//! `hub_distill`.
//!
//! Keep this module focused on reading a policy, never on writing anything.

use super::HubDistillEngine;
use clio_store::{AssocEdgeRecord, AssocKind, AssocPolicy};
use clio_types::{AmError, ErrorCode, MemoryItem};
use std::collections::BTreeMap;

impl HubDistillEngine {
    /// [`super::HubDistillEngine::eligible_hubs`] under a policy the caller
    /// already snapshotted, so one wave reads the policy once rather than once
    /// per scan.
    ///
    /// # Errors
    /// Empty bank or storage failures.
    pub(super) fn eligible_hubs_under(
        &self,
        bank: &str,
        policy: &AssocPolicy,
    ) -> Result<Vec<String>, AmError> {
        if bank.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "hub scan requires a bank",
            ));
        }
        let mut degree: BTreeMap<String, usize> = BTreeMap::new();
        for edge in self.store.list_assoc_edges(bank, None)? {
            if edge.assoc_kind != AssocKind::Coactivation {
                continue;
            }
            if self.effective(&edge, policy) < policy.w_min {
                continue;
            }
            *degree.entry(edge.source_id.clone()).or_default() += 1;
            *degree.entry(edge.target_id.clone()).or_default() += 1;
        }
        let limit = policy.hub_degree_limit as usize;
        Ok(degree
            .into_iter()
            .filter(|(_, d)| *d > limit)
            .map(|(id, _)| id)
            .collect())
    }
    /// The weight an edge counts with: explicit edges as stored, coactivation
    /// edges decayed from `last_updated_at` to now.
    pub(super) fn effective(&self, edge: &AssocEdgeRecord, policy: &AssocPolicy) -> f64 {
        match edge.assoc_kind {
            AssocKind::Explicit => edge.weight,
            AssocKind::Coactivation => clio_store::effective_weight(
                edge.weight,
                &edge.last_updated_at,
                self.now_millis(),
                policy.half_life_days,
            ),
        }
    }

    /// A hub's above-threshold neighbors, strongest first, capped at the
    /// engine's neighbor limit. Ties break on the neighbor id, so the cut is
    /// the same on every run. A discarded source never enters the cut (the
    /// limit counts claimable sources only): operations removal removes the
    /// item from active use, and a selection that re-claimed it would
    /// resurrect the removed text the cascade just withdrew.
    ///
    /// # Errors
    /// Storage failures.
    pub(super) fn top_neighbors(
        &self,
        bank: &str,
        hub_id: &str,
        policy: &AssocPolicy,
    ) -> Result<Vec<MemoryItem>, AmError> {
        let mut ranked: Vec<(f64, String)> = Vec::new();
        for edge in self.store.list_assoc_edges(bank, Some(hub_id))? {
            let eff = self.effective(&edge, policy);
            if eff < policy.w_min {
                continue;
            }
            let neighbor = if edge.source_id == hub_id {
                edge.target_id.clone()
            } else {
                edge.source_id.clone()
            };
            ranked.push((eff, neighbor));
        }
        ranked.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });
        let mut out = Vec::new();
        for (_, id) in ranked {
            if out.len() == self.neighbor_limit {
                break;
            }
            let Some(item) = self.store.get_memory_item(bank, &id)? else {
                continue;
            };
            // A discarded source must never enter a claim set — not at first
            // distillation, and not at reconcile: operations removal removes
            // the item from active use, and a reconcile that re-selected it
            // would resurrect the removed text the cascade just withdrew. The
            // limit counts claimable sources only.
            if self.store.get_item_identity(bank, &id)?.is_none() {
                continue;
            }
            out.push(item);
        }
        Ok(out)
    }
}
