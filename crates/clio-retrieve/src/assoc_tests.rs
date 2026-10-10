// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for association-engine core tests: saturating
//! reinforce on retrieve, effective decay, prune, the reinforcement rollback
//! switch, sticky explicit edges, and the EMA/isolation guard.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.

use crate::assoc::AssociationEngine;
use crate::fixtures_tests as fx;
use crate::hybrid::HybridRetriever;
use clio_store::{AssocKind, AssocPolicy, IndexStore, Store, explicit_id};
use std::sync::Arc;

#[test]
fn t01_t04_growth_applies_one_step_per_pair() {
    let store = fx::store();
    store
        .create_memory_item(&fx::item("A", "bank", "a"), "agent")
        .unwrap();
    store
        .create_memory_item(&fx::item("B", "bank", "b"), "agent")
        .unwrap();
    let eng = fx::assoc_engine_at(&store, 1_000 * fx::DAY_MILLIS);
    assert_eq!(
        eng.reinforce_hits("bank", &["A".into(), "B".into()])
            .unwrap(),
        1
    );
    let edges = store.list_assoc_edges("bank", None).unwrap();
    assert_eq!(edges.len(), 1);
    assert!((edges[0].weight - 0.15).abs() < 1e-12);
    assert_eq!(edges[0].assoc_kind, AssocKind::Coactivation);
    // Second reinforce is the second saturating step, not a reset.
    eng.reinforce_hits("bank", &["B".into(), "A".into()])
        .unwrap();
    let edges = store.list_assoc_edges("bank", None).unwrap();
    assert!((edges[0].weight - 0.2775).abs() < 1e-12);
    // Singletons are no-ops.
    assert_eq!(eng.reinforce_hits("bank", &["A".into()]).unwrap(), 0);
    assert_eq!(eng.reinforce_hits("bank", &[]).unwrap(), 0);
}

#[test]
fn t04_retrieve_reinforces_every_returned_pair() {
    let store = fx::store();
    for id in ["A", "B", "C"] {
        fx::put_lexical(
            &store,
            &fx::item(id, "bank", &format!("alpha {id}")),
            "alpha",
        );
    }
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&crate::types::RetrieveRequest::new("bank", "alpha"))
        .expect("retrieve");
    assert_eq!(out.hits.len(), 3);
    assert_eq!(out.pairs_reinforced, 3);
    let edges = store.list_assoc_edges("bank", None).unwrap();
    assert_eq!(edges.len(), 3, "3 unordered pairs");
    for e in &edges {
        assert_eq!(e.assoc_kind, AssocKind::Coactivation);
        assert!((e.weight - 0.15).abs() < 1e-12);
    }
}

#[test]
fn t08b_retrieve_expand_graph_includes_assoc_neighbor() {
    let store = fx::store();
    fx::put_lexical(&store, &fx::item("A", "bank", "alpha only"), "alpha");
    store
        .create_memory_item(&fx::item("C", "bank", "unrelated"), "agent")
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64;
    store
        .reinforce_assoc_edge("bank", "A", "C", now, 0.15)
        .unwrap();

    let retriever = fx::retriever(&store);
    let mut req = crate::types::RetrieveRequest::new("bank", "alpha");
    req.expand_graph = false;
    let off = retriever.retrieve(&req).expect("retrieve");
    let off_ids: Vec<_> = off.hits.iter().map(|h| h.hit.item_id.clone()).collect();
    assert!(!off_ids.contains(&"C".to_owned()), "off: {off_ids:?}");
    assert_eq!(off.graph_expanded, 0);

    req.expand_graph = true;
    let on = retriever.retrieve(&req).expect("retrieve");
    let on_ids: Vec<_> = on.hits.iter().map(|h| h.hit.item_id.clone()).collect();
    assert!(on_ids.contains(&"C".to_owned()), "on: {on_ids:?}");
    assert_eq!(on.graph_expanded, 1);
}

#[test]
fn t02_t03_decay_prunes_below_w_min() {
    let store = fx::store();
    let eng = fx::assoc_engine_at(&store, 0);
    eng.reinforce_hits("bank", &["A".into(), "B".into()])
        .unwrap();
    // 30 days later, half-life 30d: effective ~0.075 (0.15 * 0.5) >= w_min.
    let later = fx::assoc_engine_at(&store, 30 * fx::DAY_MILLIS);
    let views = later.associations("bank", "A", None).unwrap();
    assert_eq!(views.len(), 1);
    assert!((views[0].effective_weight - 0.075).abs() < 1e-9);
    // 90 more days: 0.15 * 2^-4 = 0.009375 < 0.05 -> pruned on observe.
    let much_later = fx::assoc_engine_at(&store, 120 * fx::DAY_MILLIS);
    let pruned = much_later.associations("bank", "A", None).unwrap();
    assert!(pruned.is_empty(), "{pruned:?}");
    let pruned_edges = store.list_assoc_edges("bank", None).unwrap();
    assert!(pruned_edges.is_empty(), "{pruned_edges:?}");
}

#[test]
fn t04b_reinforcement_switch_and_policy_replacement() {
    let store = fx::store();
    for id in ["A", "B"] {
        fx::put_lexical(
            &store,
            &fx::item(id, "bank", &format!("alpha {id}")),
            "alpha",
        );
    }
    let mut retriever = fx::retriever(&store);
    retriever.set_reinforcement(false);
    retriever
        .retrieve(&crate::types::RetrieveRequest::new("bank", "alpha"))
        .unwrap();
    let off_edges = store.list_assoc_edges("bank", None).unwrap();
    assert!(off_edges.is_empty(), "{off_edges:?}");
    retriever.set_reinforcement(true);
    retriever.set_assoc_policy(AssocPolicy {
        eta: 0.5,
        ..AssocPolicy::default()
    });
    retriever
        .retrieve(&crate::types::RetrieveRequest::new("bank", "alpha"))
        .unwrap();
    let edges = store.list_assoc_edges("bank", None).unwrap();
    assert_eq!(edges.len(), 1);
    assert!((edges[0].weight - 0.5).abs() < 1e-12);
    // Engine debug + policy accessor coverage.
    let eng = fx::assoc_engine_at(&store, 0);
    assert!(format!("{eng:?}").contains("AssociationEngine"));
    assert_eq!(eng.policy().hub_degree_limit, 16);
    eng.set_policy(AssocPolicy {
        eta: 0.2,
        ..AssocPolicy::default()
    });
    assert!((eng.policy().eta - 0.2).abs() < 1e-12);
}

#[test]
fn t05_reinforce_failure_fails_retrieve_closed() {
    // A rejected eta makes the store's reinforce validation fail after the hit
    // set is known; the retrieve call must surface the error, never return hits
    // with a silently skipped reinforce.
    let store = fx::store();
    for id in ["A", "B"] {
        fx::put_lexical(
            &store,
            &fx::item(id, "bank", &format!("alpha {id}")),
            "alpha",
        );
    }
    let retriever = fx::retriever(&store);
    retriever.set_assoc_policy(AssocPolicy {
        eta: 2.0,
        ..AssocPolicy::default()
    });
    let err = retriever
        .retrieve(&crate::types::RetrieveRequest::new("bank", "alpha"))
        .expect_err("reinforce failure must fail closed");
    assert_eq!(err.code, clio_types::ErrorCode::OutOfRange);
}

#[test]
fn t12_eta_change_scales_growth_step() {
    let store = fx::store();
    let eng = AssociationEngine::with_clock(
        Arc::clone(&store) as Arc<dyn Store>,
        AssocPolicy {
            eta: 0.5,
            ..AssocPolicy::default()
        },
        0,
    );
    eng.reinforce_hits("bank", &["A".into(), "B".into()])
        .unwrap();
    let edges = store.list_assoc_edges("bank", None).unwrap();
    assert!((edges[0].weight - 0.5).abs() < 1e-12);
}

#[test]
fn t12b_ranking_env_coactivation_wires_into_retriever() {
    // A knob set on the live RankingEnv must change reinforcement via the
    // production constructor, not only when the policy object is set directly.
    let store = fx::store();
    for id in ["A", "B"] {
        fx::put_lexical(
            &store,
            &fx::item(id, "bank", &format!("alpha {id}")),
            "alpha",
        );
    }
    let mut env = clio_config::ranking::RankingEnv::system_defaults();
    env.coactivation.eta = 0.5;
    let retriever = HybridRetriever::with_ranking_env(
        Arc::clone(&store) as Arc<dyn Store>,
        Arc::clone(&store) as Arc<dyn IndexStore>,
        &env,
    );
    retriever
        .retrieve(&crate::types::RetrieveRequest::new("bank", "alpha"))
        .expect("retrieve");
    let edges = store.list_assoc_edges("bank", None).unwrap();
    assert_eq!(edges.len(), 1);
    assert!(
        (edges[0].weight - 0.5).abs() < 1e-12,
        "ranking_env eta must drive the growth step, got {}",
        edges[0].weight
    );
}

#[test]
fn t15_aged_sticky_explicit_edge_survives() {
    let store = fx::store();
    for id in ["A", "B"] {
        store
            .create_memory_item(&fx::item(id, "bank", id), "agent")
            .unwrap();
    }
    let old = (0_i64).to_string();
    store
        .put_assoc_edge(&clio_store::AssocEdgeRecord {
            id: explicit_id("bank", "A", "B", "caused_by"),
            bank_id: "bank".into(),
            source_id: "A".into(),
            target_id: "B".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("caused_by".into()),
            weight: 0.2,
            last_updated_at: old,
        })
        .unwrap();
    // Far past any coactivation half-life: explicit stays at raw weight.
    let eng = fx::assoc_engine_at(&store, 10_000 * fx::DAY_MILLIS);
    let views = eng.associations("bank", "A", None).unwrap();
    assert_eq!(views.len(), 1);
    assert!((views[0].effective_weight - 0.2).abs() < 1e-12);
    assert_eq!(store.list_assoc_edges("bank", None).unwrap().len(), 1);
}

#[test]
fn t13_assoc_engine_has_no_ema_or_update_rule_wiring() {
    let assoc_src = include_str!("assoc.rs");
    let store_assoc_src = include_str!("../../clio-store/src/assoc.rs");
    for banned in [
        "update_rule",
        "observe_continuous",
        "clio_write",
        "Ema",
        "ema_",
    ] {
        assert!(
            !assoc_src.contains(banned),
            "assoc engine must not reference `{banned}`"
        );
        assert!(
            !store_assoc_src.contains(banned),
            "assoc policy must not reference `{banned}`"
        );
    }
}
