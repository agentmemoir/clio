// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `audit_trail` service tests: masked views,
//! lineage assembly, and not-found mapping.
//!
//! ## Owns
//! - Test assertions for the audit trail service
//!
//! ## Does not own
//! - Production reconstruction (`audit`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::audit::audit_trail;
use clio_store::{BeliefStore, LocalDevKms, SqliteStore, Store};
use clio_types::{ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

fn seeded() -> (SqliteStore, String) {
    let store = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("store");
    let bank = format!("bank-audit-{}", std::process::id());
    let item = MemoryItem {
        id: "sem_001".into(),
        bank_id: bank.clone(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::TaskSpec),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.88),
        source_ref: Some("turn-9".into()),
        snapshot: Some(json!({"deploy": "v1"})),
        gist: Some("deploys nightly".into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    };
    store.create_memory_item(&item, "agent").expect("seed");
    (store, bank)
}

#[test]
fn belief_trail_carries_masked_trajectory() {
    let (store, bank) = seeded();
    let belief = clio_types::BeliefObject {
        context: None,
        context_as_of: None,
        id: "bel-1".into(),
        bank_id: bank.clone(),
        subject_id: "subj-bel".into(),
        proposition: "User is likely vegetarian".into(),
        confidence_history: vec![
            clio_types::ConfidenceEntry::new(
                "2024-01-01",
                0.4,
                "turn_112",
                clio_types::SourceType::AgentInferred,
            )
            .expect("entry"),
        ],
        created_at: "2024-01-01".into(),
        updated_at: "2024-01-01".into(),
    };
    store.create_belief(&belief, "agent").expect("belief");
    store
        .append_confidence(
            &bank,
            "bel-1",
            &clio_types::ConfidenceEntry::new(
                "2024-02-01",
                0.65,
                "turn_340",
                clio_types::SourceType::UserStated,
            )
            .expect("entry"),
            "agent",
        )
        .expect("append");
    let view = audit_trail(&store, &bank, "bel-1").expect("trail");
    assert_eq!(view.lineage_ids, vec!["bel-1".to_owned()]);
    assert_eq!(view.belief_confidence.len(), 2);
    assert_eq!(view.belief_confidence[0]["confidence"], json!(0.4));
    assert_eq!(view.belief_confidence[1]["confidence"], json!(0.65));
    assert!(view.revisions.is_empty(), "{:?}", view.revisions);
    // The merged timeline carries both belief points and the belief telemetry
    // events, ordered ascending.
    assert_eq!(
        view.timeline
            .iter()
            .filter(|t| t["kind"] == "belief")
            .count(),
        2
    );
    let ts: Vec<&str> = view
        .timeline
        .iter()
        .filter_map(|t| t["timestamp"].as_str())
        .collect();
    let mut sorted = ts.clone();
    sorted.sort_unstable();
    assert_eq!(ts, sorted, "timeline must be ascending");
}

#[test]
fn timeline_merges_revisions_and_events_in_order() {
    let (store, bank) = seeded();
    let view = audit_trail(&store, &bank, "sem_001").expect("trail");
    assert_eq!(
        view.timeline.len(),
        view.revisions.len() + view.events.len() + view.belief_confidence.len()
    );
    let kinds: Vec<&str> = view
        .timeline
        .iter()
        .filter_map(|t| t["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"revision"), "{kinds:?}");
    assert!(kinds.contains(&"event"), "{kinds:?}");
    let ts: Vec<&str> = view
        .timeline
        .iter()
        .filter_map(|t| t["timestamp"].as_str())
        .collect();
    let mut sorted = ts.clone();
    sorted.sort_unstable();
    assert_eq!(ts, sorted, "timeline must be ascending");
}

#[test]
fn t18_02_trail_masks_secrets_and_maps_not_found() {
    let (store, bank) = seeded();
    // Missing id maps to the typed error.
    let err = audit_trail(&store, &bank, "missing").expect_err("not found");
    assert_eq!(err.code, ErrorCode::ItemNotFound);
    // Empty scope args fail closed before the store call.
    assert_eq!(
        audit_trail(&store, "", "x").expect_err("empty bank").code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        audit_trail(&store, &bank, "").expect_err("empty id").code,
        ErrorCode::InvalidArgument
    );
    let view = audit_trail(&store, &bank, "sem_001").expect("trail");
    assert_eq!(view.item_id, "sem_001");
    assert_eq!(view.lineage_ids, vec!["sem_001".to_owned()]);
    assert_eq!(view.revisions.len(), 1);
    assert!(
        view.events
            .iter()
            .any(|e| e["operation"] == "create_memory_item")
    );
    let ev = view.events.first().expect("creation event");
    assert_eq!(ev["actor"], "agent");
    assert_eq!(ev["admission_score"], 0.88);
}
