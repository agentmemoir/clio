// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the in-process JSON tool surface for the
//! association tools: `associations`, `graph_link`, and `graph_query`.
//!
//! ## Owns
//! - Wire argument structs and `*_from_json` handlers returning `ToolResult`
//!
//! ## Does not own
//! - The association engine / traversal (`assoc`); MCP transport binding (later work)
//!
//! ## Boundary
//!
//! Changes to the association JSON contract should normally require changes here.
//! Changes to traversal internals should not require changes here.

use crate::assoc::{AssocEdgeView, AssociationEngine, GraphNodeView};
use clio_store::AssocKind;
use clio_types::{AmError, ErrorCode, ToolResult};
use serde::Deserialize;
use serde_json::Value;

/// Arguments for the `associations` tool.
#[derive(Debug, Deserialize)]
pub struct AssociationsArgs {
    /// Bank isolation key (session-injected, like `retrieve`).
    pub bank: String,
    /// Item whose neighbors are listed.
    pub item_id: String,
    /// Optional minimum effective weight (default [`clio_store::AssocPolicy::w_min`]).
    #[serde(default)]
    pub min_weight: Option<f64>,
}

/// Arguments for the `graph_link` tool.
#[derive(Debug, Deserialize)]
pub struct GraphLinkArgs {
    /// Bank isolation key.
    pub bank: String,
    /// Source item id (must exist in the bank).
    pub source_id: String,
    /// Target item id (must exist in the bank).
    pub target_id: String,
    /// Explicit relationship label (`relationship`, not a triple predicate).
    pub relationship: String,
    /// Optional initial weight (default `1.0`).
    #[serde(default)]
    pub weight: Option<f64>,
    /// Optional attribution actor.
    #[serde(default)]
    pub actor: Option<String>,
}

/// Arguments for the `graph_query` tool.
#[derive(Debug, Deserialize)]
pub struct GraphQueryArgs {
    /// Bank isolation key.
    pub bank: String,
    /// Seed item id.
    pub seed_id: String,
    /// Optional hop cap (default: policy `max_hops`).
    #[serde(default)]
    pub max_hops: Option<usize>,
    /// Optional `assoc_kind` filter: `coactivation` or `explicit`.
    #[serde(default)]
    pub edge_type: Option<String>,
    /// Optional minimum effective edge weight.
    #[serde(default)]
    pub min_weight: Option<f64>,
}

/// Dispatch `associations(item_id, min_weight?)`.
#[must_use]
pub fn associations_from_json(
    engine: &AssociationEngine,
    args: &Value,
) -> ToolResult<Vec<AssocEdgeView>> {
    let parsed: AssociationsArgs = match serde_json::from_value(args.clone()) {
        Ok(parsed) => parsed,
        Err(e) => return ToolResult::failure(invalid(format!("associations args: {e}"))),
    };
    ToolResult::from_result(engine.associations(&parsed.bank, &parsed.item_id, parsed.min_weight))
}

/// Dispatch `graph_link(source_id, target_id, relationship, weight?)`.
#[must_use]
pub fn graph_link_from_json(engine: &AssociationEngine, args: &Value) -> ToolResult<AssocEdgeView> {
    let parsed: GraphLinkArgs = match serde_json::from_value(args.clone()) {
        Ok(parsed) => parsed,
        Err(e) => return ToolResult::failure(invalid(format!("graph_link args: {e}"))),
    };
    let actor = parsed.actor.as_deref().unwrap_or("system");
    ToolResult::from_result(engine.graph_link(
        &parsed.bank,
        &parsed.source_id,
        &parsed.target_id,
        &parsed.relationship,
        parsed.weight,
        actor,
    ))
}

/// Dispatch `graph_query(seed_id, max_hops?, edge_type?, min_weight?)`.
#[must_use]
pub fn graph_query_from_json(
    engine: &AssociationEngine,
    args: &Value,
) -> ToolResult<Vec<GraphNodeView>> {
    let parsed: GraphQueryArgs = match serde_json::from_value(args.clone()) {
        Ok(parsed) => parsed,
        Err(e) => return ToolResult::failure(invalid(format!("graph_query args: {e}"))),
    };
    let edge_type = match parsed.edge_type.as_deref() {
        None | Some("") => None,
        Some(raw) => match AssocKind::parse(raw) {
            Ok(kind) => Some(kind),
            Err(e) => return ToolResult::failure(e),
        },
    };
    ToolResult::from_result(engine.graph_query(
        &parsed.bank,
        &parsed.seed_id,
        parsed.max_hops,
        edge_type,
        parsed.min_weight,
    ))
}

fn invalid(message: String) -> AmError {
    AmError::new(ErrorCode::InvalidArgument, message)
}

#[cfg(test)]
#[path = "assoc_surface_tests.rs"]
mod assoc_surface_tests;
