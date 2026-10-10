// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the JSON tool-surface tests for
//! `associations`, `graph_link`, and `graph_query`.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.

use crate::assoc::AssociationEngine;
use crate::assoc_surface::{associations_from_json, graph_link_from_json, graph_query_from_json};
use crate::fixtures_tests as fx;
use clio_store::{AssocPolicy, Store};
use clio_types::{ErrorCode, ToolResult};
use serde_json::json;
use std::sync::Arc;

fn engine(store: &Arc<clio_store::SqliteStore>) -> AssociationEngine {
    AssociationEngine::with_clock(
        Arc::clone(store) as Arc<dyn Store>,
        AssocPolicy::default(),
        0,
    )
}

fn seed_items(store: &Arc<clio_store::SqliteStore>) {
    for id in ["A", "B", "C"] {
        store
            .create_memory_item(&fx::item(id, "bank", id), "agent")
            .unwrap();
    }
}

#[test]
fn associations_surface_lists_and_validates() {
    let store = fx::store();
    seed_items(&store);
    let eng = engine(&store);
    let ToolResult::Ok { data, .. } = graph_link_from_json(
        &eng,
        &json!({"bank": "bank", "source_id": "A", "target_id": "B", "relationship": "caused_by"}),
    ) else {
        panic!("graph_link should succeed");
    };
    assert_eq!(data.neighbor_id, "B");
    assert!((data.effective_weight - 1.0).abs() < 1e-12);

    let ToolResult::Ok { data: views, .. } =
        associations_from_json(&eng, &json!({"bank": "bank", "item_id": "A"}))
    else {
        panic!("associations should succeed");
    };
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].relationship.as_deref(), Some("caused_by"));

    // min_weight filters, missing args fail, bad item fails.
    let ToolResult::Ok { data: none, .. } = associations_from_json(
        &eng,
        &json!({"bank": "bank", "item_id": "A", "min_weight": 1.5}),
    ) else {
        panic!("associations should succeed");
    };
    assert!(none.is_empty(), "{none:?}");
    assert!(matches!(
        associations_from_json(&eng, &json!({"bank": "bank"})),
        ToolResult::Err {
            code: ErrorCode::InvalidArgument,
            ..
        }
    ));
    assert!(matches!(
        associations_from_json(&eng, &json!({"bank": "bank", "item_id": ""})),
        ToolResult::Err {
            code: ErrorCode::InvalidArgument,
            ..
        }
    ));
}

#[test]
fn graph_link_surface_rejects_missing_endpoint() {
    let store = fx::store();
    seed_items(&store);
    let eng = engine(&store);
    assert!(matches!(
        graph_link_from_json(
            &eng,
            &json!({"bank": "bank", "source_id": "A", "target_id": "ghost", "relationship": "r"}),
        ),
        ToolResult::Err {
            code: ErrorCode::UnknownPath,
            ..
        }
    ));
    assert!(matches!(
        graph_link_from_json(&eng, &json!({"bank": "bank"})),
        ToolResult::Err {
            code: ErrorCode::InvalidArgument,
            ..
        }
    ));
}

#[test]
fn graph_query_surface_hops_and_filters() {
    let store = fx::store();
    seed_items(&store);
    let eng = engine(&store);
    eng.graph_link("bank", "A", "B", "caused_by", None, "agent")
        .unwrap();
    eng.graph_link("bank", "B", "C", "depends_on", None, "agent")
        .unwrap();

    let ToolResult::Ok { data: hops1, .. } = graph_query_from_json(
        &eng,
        &json!({"bank": "bank", "seed_id": "A", "max_hops": 1}),
    ) else {
        panic!("graph_query should succeed");
    };
    assert_eq!(hops1.len(), 1);
    assert_eq!(hops1[0].item_id, "B");

    let ToolResult::Ok { data: hops2, .. } = graph_query_from_json(
        &eng,
        &json!({"bank": "bank", "seed_id": "A", "max_hops": 2}),
    ) else {
        panic!("graph_query should succeed");
    };
    assert_eq!(hops2.len(), 2);

    // edge_type filter is parsed to assoc_kind; unknown values fail.
    let ToolResult::Ok { data: coact, .. } = graph_query_from_json(
        &eng,
        &json!({"bank": "bank", "seed_id": "A", "edge_type": "coactivation"}),
    ) else {
        panic!("graph_query should succeed");
    };
    assert!(coact.is_empty(), "{coact:?}");
    assert!(matches!(
        graph_query_from_json(
            &eng,
            &json!({"bank": "bank", "seed_id": "A", "edge_type": "class"}),
        ),
        ToolResult::Err {
            code: ErrorCode::InvalidArgument,
            ..
        }
    ));
    assert!(matches!(
        graph_query_from_json(&eng, &json!({"bank": "bank", "seed_id": ""})),
        ToolResult::Err {
            code: ErrorCode::InvalidArgument,
            ..
        }
    ));
}
