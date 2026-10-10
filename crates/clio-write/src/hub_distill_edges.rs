// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `hub_distill` wave's write-outcome
//! helpers: the explicit cross-reference edges a distilled unit writes to its
//! hub and sources, the optional post-distill cluster-edge prune, and the
//! per-item anonymized-derivative audit row.
//!
//! ## Owns
//! - `HubDistillEngine::cross_reference`, `prune_cluster`, `audit`
//!
//! ## Does not own
//! - The distill pipeline and wave envelope (`hub_distill`)
//! - Scans (`hub_distill_scan`) and reconcile decisions (`hub_distill_reconcile`)
//! - Assoc-edge storage (`clio-store`)
//!
//! ## Boundary
//!
//! Changes to what a distilled unit writes about its outcome should normally
//! require changes here. Changes to when a wave runs should not require
//! changes here.
//!
//! Keep this module focused on the distill write-outcome helpers.
//!
//! Split out of `hub_distill.rs` to keep that file inside the 450-line limit.

use super::HubDistillEngine;
use clio_store::{AssocEdgeRecord, AssocKind, explicit_id};
use clio_types::{AmError, MemoryItem, OpContext};

impl HubDistillEngine {
    /// Write the explicit cross-reference edges for a distilled unit (hub
    /// weight 1.0, each source 0.5) and record the PII-safe audit row.
    pub(super) fn cross_reference(
        &self,
        bank: &str,
        item_id: &str,
        hub_id: &str,
        neighbors: &[MemoryItem],
        ctx: &OpContext,
    ) -> Result<(), AmError> {
        let mut endpoints: Vec<(String, f64)> = vec![(hub_id.to_owned(), 1.0)];
        endpoints.extend(neighbors.iter().map(|n| (n.id.clone(), 0.5)));
        let count = crate::hub_distill_model::endpoints_count(item_id, hub_id, neighbors);
        for (endpoint, weight) in endpoints {
            if endpoint == item_id {
                continue;
            }
            let record = AssocEdgeRecord {
                id: explicit_id(bank, item_id, &endpoint, super::HUB_DISTILL_JOB),
                bank_id: bank.to_owned(),
                source_id: item_id.to_owned(),
                target_id: endpoint,
                assoc_kind: AssocKind::Explicit,
                relationship: Some(super::HUB_DISTILL_JOB.to_owned()),
                weight,
                last_updated_at: self.now_millis().to_string(),
            };
            self.store.put_assoc_edge(&record)?;
        }
        self.store.record_assoc_audit(
            bank,
            "hub_distill_cross_ref",
            Some(item_id),
            ctx.actor.as_str(),
            &serde_json::json!({"hub_id": hub_id, "endpoints": count}),
        )
    }

    /// Delete the cluster's co-activation edges after a successful distill
    /// (the optional prune), and record the audit row.
    pub(super) fn prune_cluster(
        &self,
        bank: &str,
        hub_id: &str,
        ctx: &OpContext,
    ) -> Result<(), AmError> {
        let mut pruned = 0_u64;
        for edge in self.store.list_assoc_edges(bank, Some(hub_id))? {
            if edge.assoc_kind == AssocKind::Coactivation
                && self.store.delete_assoc_edge(bank, &edge.id)?
            {
                pruned += 1;
            }
        }
        self.store.record_assoc_audit(
            bank,
            "hub_distill_prune",
            Some(hub_id),
            ctx.actor.as_str(),
            &serde_json::json!({"hub_id": hub_id, "pruned_edges": pruned}),
        )
    }

    /// The per-item anonymized-derivative determination audit row: ids and
    /// counts only, and a failed write propagates so the distill surfaces an
    /// error instead of silently omitting the decision record.
    pub(super) fn audit(
        &self,
        bank: &str,
        hub_id: &str,
        item_id: &str,
        neighbors: &[MemoryItem],
        admitted: bool,
        ctx: &OpContext,
    ) -> Result<(), AmError> {
        let detail = serde_json::json!({
            "hub_id": hub_id,
            "item_id": item_id,
            "admitted": admitted,
            "neighbor_count": neighbors.len(),
            "derived": true,
        });
        self.store.record_assoc_audit(
            bank,
            "hub_distill",
            Some(item_id),
            ctx.actor.as_str(),
            &detail,
        )
    }
}
