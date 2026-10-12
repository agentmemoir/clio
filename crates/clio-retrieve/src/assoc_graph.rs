// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the association tool traversal surface:
//! `associations`, explicit `graph_link` edges, bounded `graph_query`, and
//! `expand_graph` neighbor lookup.
//!
//! ## Owns
//! - `impl AssociationEngine` traversal/link methods and `StoreGraphExpander`
//!
//! ## Does not own
//! - The engine core / reinforce / observe (`assoc`) or storage (`clio-store`)
//!
//! ## Boundary
//!
//! Changes to association tool traversal semantics should normally require
//! changes here. Changes to the weight formula should not require changes here.

use crate::assoc::{AssocEdgeView, AssociationEngine, DEFAULT_QUERY_NODE_CAP, GraphNodeView};
use crate::types::GraphExpander;
use clio_store::{AssocEdgeRecord, AssocKind, canonical_pair, explicit_id};
use clio_types::{AmError, ErrorCode};
use std::collections::BTreeSet;
use std::sync::Arc;

impl AssociationEngine {
    /// List neighbors of `item_id` with effective weights (prune-on-observe).
    ///
    /// # Errors
    /// Empty bank/item or storage failures.
    pub fn associations(
        &self,
        bank: &str,
        item_id: &str,
        min_weight: Option<f64>,
    ) -> Result<Vec<AssocEdgeView>, AmError> {
        if bank.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "associations requires a bank",
            ));
        }
        if item_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "associations requires an item id",
            ));
        }
        let policy = self.policy();
        let min = min_weight.unwrap_or(policy.w_min);
        let mut out = Vec::new();
        for edge in self.store.list_assoc_edges(bank, Some(item_id))? {
            if edge.source_id == edge.target_id {
                continue;
            }
            let Some(eff) = self.observe(bank, &edge, &policy)? else {
                continue;
            };
            if eff < min {
                continue;
            }
            out.push(AssocEdgeView {
                neighbor_id: other_endpoint(&edge, item_id),
                edge_id: edge.id,
                source_id: edge.source_id,
                target_id: edge.target_id,
                assoc_kind: edge.assoc_kind,
                relationship: edge.relationship,
                weight: edge.weight,
                effective_weight: eff,
                last_reinforced_at: edge.last_updated_at,
            });
        }
        out.sort_by(|a, b| {
            b.effective_weight
                .partial_cmp(&a.effective_weight)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.neighbor_id.cmp(&b.neighbor_id))
                .then_with(|| a.relationship.cmp(&b.relationship))
        });
        Ok(out)
    }

    /// Create/update an explicit `graph_link` edge keyed by relationship.
    ///
    /// # Errors
    /// Missing endpoints in the bank, invalid weight/relationship, or storage failures.
    pub fn graph_link(
        &self,
        bank: &str,
        source_id: &str,
        target_id: &str,
        relationship: &str,
        weight: Option<f64>,
        actor: &str,
    ) -> Result<AssocEdgeView, AmError> {
        if bank.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "graph_link requires a bank",
            ));
        }
        if source_id.is_empty() || target_id.is_empty() || relationship.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "graph_link requires source_id, target_id, and relationship",
            ));
        }
        if source_id == target_id {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "graph_link requires two distinct item ids",
            ));
        }
        let w = weight.unwrap_or(1.0);
        if !(0.0..=1.0).contains(&w) {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "graph_link weight must be in [0, 1]",
            ));
        }
        for id in [source_id, target_id] {
            if self.store.get_memory_item(bank, id)?.is_none() {
                return Err(AmError::new(
                    ErrorCode::ItemNotFound,
                    format!("graph_link endpoint `{id}` not found in bank"),
                ));
            }
        }
        let (lo, hi) = canonical_pair(source_id, target_id);
        let edge = AssocEdgeRecord {
            id: explicit_id(bank, source_id, target_id, relationship),
            bank_id: bank.to_owned(),
            source_id: lo,
            target_id: hi,
            assoc_kind: AssocKind::Explicit,
            relationship: Some(relationship.to_owned()),
            weight: w,
            last_updated_at: self.now_millis().to_string(),
        };
        self.store.put_assoc_edge(&edge)?;
        self.store.record_assoc_audit(
            bank,
            "graph_link",
            Some(&edge.id),
            actor,
            &serde_json::json!({
                "edge_id": edge.id,
                "relationship": relationship,
                "weight": w,
            }),
        )?;
        Ok(AssocEdgeView {
            neighbor_id: target_id.to_owned(),
            edge_id: edge.id,
            source_id: edge.source_id,
            target_id: edge.target_id,
            assoc_kind: AssocKind::Explicit,
            relationship: edge.relationship,
            weight: w,
            effective_weight: w,
            last_reinforced_at: edge.last_updated_at,
        })
    }

    /// Bounded multi-hop traversal from `seed`, filtered by kind and weight.
    ///
    /// # Errors
    /// Empty bank/seed or storage failures.
    pub fn graph_query(
        &self,
        bank: &str,
        seed_id: &str,
        max_hops: Option<usize>,
        edge_type: Option<AssocKind>,
        min_weight: Option<f64>,
    ) -> Result<Vec<GraphNodeView>, AmError> {
        if bank.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "graph_query requires a bank",
            ));
        }
        if seed_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "graph_query requires a seed id",
            ));
        }
        let policy = self.policy();
        let hops = max_hops.unwrap_or(policy.max_hops as usize);
        if hops == 0 {
            return Ok(Vec::new());
        }
        let min = min_weight.unwrap_or(policy.w_min);
        let mut visited: BTreeSet<String> = BTreeSet::from([seed_id.to_owned()]);
        let mut frontier = vec![seed_id.to_owned()];
        let mut out = Vec::new();
        for hop in 1..=hops {
            let mut next = Vec::new();
            for node in &frontier {
                for edge in self.store.list_assoc_edges(bank, Some(node))? {
                    if matches!(edge_type, Some(kind) if kind != edge.assoc_kind) {
                        continue;
                    }
                    let Some(eff) = self.observe(bank, &edge, &policy)? else {
                        continue;
                    };
                    if eff < min {
                        continue;
                    }
                    let neighbor = other_endpoint(&edge, node);
                    if visited.insert(neighbor.clone()) {
                        out.push(GraphNodeView {
                            item_id: neighbor.clone(),
                            hop,
                            via_edge_id: edge.id.clone(),
                            via_source_id: node.clone(),
                            effective_weight: eff,
                        });
                        next.push(neighbor);
                        if out.len() >= DEFAULT_QUERY_NODE_CAP {
                            return Ok(out);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        Ok(out)
    }

    /// Bounded neighbor ids reachable from `seeds` (used by `expand_graph`).
    ///
    /// # Errors
    /// Empty bank or storage failures.
    pub fn expand_ids(
        &self,
        bank: &str,
        seeds: &[String],
        max_hops: usize,
    ) -> Result<Vec<String>, AmError> {
        if bank.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "expand requires a bank",
            ));
        }
        let policy = self.policy();
        let mut capped: Vec<String> = Vec::new();
        for seed in seeds {
            if !seed.is_empty() {
                capped.push(seed.clone());
            }
            if capped.len() >= policy.expand_seed_cap {
                break;
            }
        }
        if capped.is_empty() {
            return Ok(Vec::new());
        }
        let mut visited: BTreeSet<String> = capped.iter().cloned().collect();
        let mut frontier = capped;
        let mut out = Vec::new();
        for _ in 0..max_hops {
            let mut next = Vec::new();
            for node in &frontier {
                for edge in self.store.list_assoc_edges(bank, Some(node))? {
                    // Expansion honors `w_min` for every kind: `observe` returns
                    // the effective weight (raw for sticky explicit edges) and
                    // prunes decayed coactivation edges below it.
                    let Some(eff) = self.observe(bank, &edge, &policy)? else {
                        continue;
                    };
                    if eff < policy.w_min {
                        continue;
                    }
                    let neighbor = other_endpoint(&edge, node);
                    if visited.insert(neighbor.clone()) {
                        out.push(neighbor.clone());
                        next.push(neighbor);
                        if out.len() >= policy.expand_neighbor_cap {
                            return Ok(out);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        Ok(out)
    }
}

/// The endpoint of `edge` opposite `node` (stored source when `node` is target).
fn other_endpoint(edge: &AssocEdgeRecord, node: &str) -> String {
    if edge.source_id == node {
        edge.target_id.clone()
    } else {
        edge.source_id.clone()
    }
}

/// `GraphExpander` adapter over the association engine.
pub struct StoreGraphExpander {
    engine: Arc<AssociationEngine>,
}

impl StoreGraphExpander {
    /// Wrap an association engine.
    #[must_use]
    pub fn new(engine: Arc<AssociationEngine>) -> Self {
        Self { engine }
    }
}

impl GraphExpander for StoreGraphExpander {
    fn expand(
        &self,
        bank: &str,
        seeds: &[String],
        max_hops: usize,
    ) -> Result<Vec<String>, AmError> {
        self.engine.expand_ids(bank, seeds, max_hops)
    }
}

#[cfg(test)]
#[path = "assoc_graph_tests.rs"]
mod assoc_graph_tests;
