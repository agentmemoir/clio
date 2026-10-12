// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for association tool traversal tests:
//! `associations`, explicit `graph_link` multi-relationship, bounded
//! `graph_query`, `expand_graph` caps, validation, and bank isolation.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.

use crate::assoc::StoreGraphExpander;
use crate::fixtures_tests as fx;
use crate::types::GraphExpander;
use clio_store::{AssocKind, Store};
use std::sync::Arc;

#[test]
fn t06_graph_link_two_relationships_both_listed() {
    let store = fx::store();
    for id in ["A", "B"] {
        store
            .create_memory_item(&fx::item(id, "bank", id), "agent")
            .unwrap();
    }
    let eng = fx::assoc_engine_at(&store, 0);
    eng.graph_link("bank", "A", "B", "caused_by", None, "agent")
        .unwrap();
    eng.graph_link("bank", "A", "B", "depends_on", Some(0.4), "agent")
        .unwrap();
    let views = eng.associations("bank", "A", None).unwrap();
    assert_eq!(views.len(), 2);
    let rels: Vec<_> = views
        .iter()
        .filter_map(|v| v.relationship.clone())
        .collect();
    assert!(rels.contains(&"caused_by".to_owned()));
    assert!(rels.contains(&"depends_on".to_owned()));
    assert!(views.iter().all(|v| v.assoc_kind == AssocKind::Explicit));
    // Both edges reached by graph_query too.
    let nodes = eng.graph_query("bank", "A", Some(1), None, None).unwrap();
    assert_eq!(nodes.len(), 1);
}

#[test]
fn associations_tie_break_is_deterministic() {
    let store = fx::store();
    for id in ["A", "B"] {
        store
            .create_memory_item(&fx::item(id, "bank", id), "agent")
            .unwrap();
    }
    let eng = fx::assoc_engine_at(&store, 0);
    // Equal weights + equal neighbor force both sort tie-breakers to run.
    eng.graph_link("bank", "A", "B", "aaa", None, "agent")
        .unwrap();
    eng.graph_link("bank", "A", "B", "zzz", None, "agent")
        .unwrap();
    let views = eng.associations("bank", "A", None).unwrap();
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].relationship.as_deref(), Some("aaa"));
    assert_eq!(views[1].relationship.as_deref(), Some("zzz"));
}

#[test]
fn t07_graph_query_respects_max_hops() {
    let store = fx::store();
    for id in ["A", "B", "C"] {
        store
            .create_memory_item(&fx::item(id, "bank", id), "agent")
            .unwrap();
    }
    let eng = fx::assoc_engine_at(&store, 0);
    eng.reinforce_hits("bank", &["A".into(), "B".into()])
        .unwrap();
    eng.reinforce_hits("bank", &["B".into(), "C".into()])
        .unwrap();
    let one = eng.graph_query("bank", "A", Some(1), None, None).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].item_id, "B");
    let two = eng.graph_query("bank", "A", Some(2), None, None).unwrap();
    let ids: Vec<_> = two.iter().map(|n| n.item_id.clone()).collect();
    assert!(ids.contains(&"B".to_owned()) && ids.contains(&"C".to_owned()));
    // edge_type filter excludes coactivation.
    let explicit_only = eng
        .graph_query("bank", "A", Some(2), Some(AssocKind::Explicit), None)
        .unwrap();
    assert!(explicit_only.is_empty(), "{explicit_only:?}");
    // max_hops = 0 returns the seed only (empty result).
    let zero_hops = eng.graph_query("bank", "A", Some(0), None, None).unwrap();
    assert!(zero_hops.is_empty(), "{zero_hops:?}");
}

#[test]
fn t08_expand_graph_surfaces_assoc_neighbor() {
    let store = fx::store();
    fx::put_lexical(&store, &fx::item("A", "bank", "alpha only"), "alpha");
    store
        .create_memory_item(&fx::item("C", "bank", "unrelated"), "agent")
        .unwrap();
    let eng = fx::assoc_engine_at(&store, 0);
    eng.reinforce_hits("bank", &["A".into(), "C".into()])
        .unwrap();
    let expander = StoreGraphExpander::new(Arc::new(fx::assoc_engine_at(&store, 0)));
    let seeds = vec!["A".to_owned()];
    assert_eq!(
        expander.expand("bank", &seeds, 2).unwrap(),
        vec!["C".to_owned()]
    );
    // Missing store / no edges stays a no-op.
    let no_edges = expander.expand("empty-bank", &seeds, 2).unwrap();
    assert!(no_edges.is_empty(), "{no_edges:?}");
}

#[test]
fn graph_query_edge_type_filter_skips_other_kinds() {
    let store = fx::store();
    for id in ["A", "B"] {
        store
            .create_memory_item(&fx::item(id, "bank", id), "agent")
            .unwrap();
    }
    let eng = fx::assoc_engine_at(&store, 0);
    eng.graph_link("bank", "A", "B", "caused_by", None, "agent")
        .unwrap();
    // Explicit edge is skipped when asking for coactivation...
    let coactivation = eng
        .graph_query("bank", "A", Some(1), Some(AssocKind::Coactivation), None)
        .unwrap();
    assert!(coactivation.is_empty(), "{coactivation:?}");
    // ...and kept when asking for explicit.
    assert_eq!(
        eng.graph_query("bank", "A", Some(1), Some(AssocKind::Explicit), None)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn t08c_expand_skips_explicit_below_min_weight() {
    let store = fx::store();
    for id in ["A", "B", "C"] {
        store
            .create_memory_item(&fx::item(id, "bank", id), "agent")
            .unwrap();
    }
    let eng = fx::assoc_engine_at(&store, 0);
    // A weak explicit edge below the policy `w_min` must not expand...
    eng.graph_link("bank", "A", "B", "weak", Some(0.01), "agent")
        .unwrap();
    // ...while a strong explicit edge still does.
    eng.graph_link("bank", "A", "C", "strong", Some(0.5), "agent")
        .unwrap();
    let seeds = vec!["A".to_owned()];
    let expanded = eng.expand_ids("bank", &seeds, 1).unwrap();
    assert!(
        !expanded.contains(&"B".to_owned()),
        "weak explicit edge leaked past w_min: {expanded:?}"
    );
    assert!(
        expanded.contains(&"C".to_owned()),
        "strong explicit edge missing: {expanded:?}"
    );
}

#[test]
fn t11_cross_bank_link_rejected() {
    let store = fx::store();
    store
        .create_memory_item(&fx::item("A", "bank-a", "a"), "agent")
        .unwrap();
    store
        .create_memory_item(&fx::item("B", "bank-b", "b"), "agent")
        .unwrap();
    let eng = fx::assoc_engine_at(&store, 0);
    // B does not exist in bank-a, so the cross-bank link is rejected.
    let err = eng
        .graph_link("bank-a", "A", "B", "caused_by", None, "agent")
        .expect_err("cross-bank endpoint must be rejected");
    assert_eq!(err.code, clio_types::ErrorCode::ItemNotFound);
    // Self links and empty relationships are rejected too.
    assert!(
        eng.graph_link("bank-a", "A", "A", "x", None, "agent")
            .is_err()
    );
    assert!(
        eng.graph_link("bank-a", "A", "B", "", None, "agent")
            .is_err()
    );
    assert!(
        eng.graph_link("bank-a", "A", "B", "x", Some(2.0), "agent")
            .is_err()
    );
}

#[test]
fn assoc_graph_validation_and_self_loops() {
    let store = fx::store();
    store
        .create_memory_item(&fx::item("A", "bank", "a"), "agent")
        .unwrap();
    store
        .create_memory_item(&fx::item("B", "bank", "b"), "agent")
        .unwrap();
    let eng = fx::assoc_engine_at(&store, 0);
    assert_eq!(
        eng.associations("", "A", None).unwrap_err().code,
        clio_types::ErrorCode::MissingBank
    );
    assert_eq!(
        eng.graph_link("", "A", "B", "r", None, "agent")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::MissingBank
    );
    assert_eq!(
        eng.graph_query("", "A", None, None, None).unwrap_err().code,
        clio_types::ErrorCode::MissingBank
    );
    assert_eq!(
        eng.graph_query("bank", "", None, None, None)
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
    assert_eq!(
        eng.expand_ids("", &["A".into()], 2).unwrap_err().code,
        clio_types::ErrorCode::MissingBank
    );
    let from_empty_id = eng.expand_ids("bank", &[String::new()], 2).unwrap();
    assert!(from_empty_id.is_empty(), "{from_empty_id:?}");
    // Seed cap stops the scan once enough seeds are collected.
    let many: Vec<String> = (0..40).map(|i| format!("s{i}")).collect();
    let capped_scan = eng.expand_ids("bank", &many, 1).unwrap();
    assert!(capped_scan.is_empty(), "{capped_scan:?}");
    // A legacy self-loop row is skipped by associations.
    store
        .put_assoc_edge(&clio_store::AssocEdgeRecord {
            id: "self-loop".into(),
            bank_id: "bank".into(),
            source_id: "A".into(),
            target_id: "A".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("self".into()),
            weight: 0.9,
            last_updated_at: "0".into(),
        })
        .unwrap();
    let after_self_loop = eng.associations("bank", "A", None).unwrap();
    assert!(after_self_loop.is_empty(), "{after_self_loop:?}");
}

#[test]
fn graph_query_min_weight_and_node_cap() {
    let store = fx::store();
    for i in 0..=260 {
        store
            .create_memory_item(&fx::item(&format!("n{i}"), "bank", "n"), "agent")
            .unwrap();
    }
    for i in 0..260 {
        store
            .reinforce_assoc_edge("bank", &format!("n{i}"), &format!("n{}", i + 1), 0, 0.15)
            .unwrap();
    }
    let eng = fx::assoc_engine_at(&store, 0);
    // min_weight above the single-step weight filters everything out.
    let heavy = eng
        .graph_query("bank", "n0", Some(5), None, Some(0.9))
        .unwrap();
    assert!(heavy.is_empty(), "{heavy:?}");
    // The traversal stops at the hard node cap.
    let capped = eng
        .graph_query("bank", "n0", Some(500), None, None)
        .unwrap();
    assert_eq!(capped.len(), crate::assoc::DEFAULT_QUERY_NODE_CAP);
}

#[test]
fn expand_caps_and_skips_decayed_edges() {
    let store = fx::store();
    store
        .create_memory_item(&fx::item("H", "bank", "hub"), "agent")
        .unwrap();
    for i in 0..70 {
        let id = format!("e{i}");
        store
            .create_memory_item(&fx::item(&id, "bank", "n"), "agent")
            .unwrap();
        store
            .reinforce_assoc_edge("bank", "H", &id, 0, 0.15)
            .unwrap();
    }
    let eng = fx::assoc_engine_at(&store, 0);
    let seeds = vec!["H".to_owned()];
    assert_eq!(eng.expand_ids("bank", &seeds, 1).unwrap().len(), 64);
    // A decayed edge is pruned on observe and never expands.
    store
        .create_memory_item(&fx::item("old", "bank", "old"), "agent")
        .unwrap();
    store
        .reinforce_assoc_edge("bank", "H", "old", 0, 0.5)
        .unwrap();
    let far_future = fx::assoc_engine_at(&store, 10_000 * fx::DAY_MILLIS);
    let expanded = far_future.expand_ids("bank", &seeds, 1).unwrap();
    assert!(!expanded.contains(&"old".to_owned()));
}
