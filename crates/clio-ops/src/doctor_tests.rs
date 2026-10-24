// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the `doctor` read-only planner:
//! finding-to-action composition, registry behavior, and the action allowlist.
//!
//! ## Owns
//! - `doctor` plan shape and composed actions/notes across finding sets
//! - `PlanRegistry` id allocation, insert, get, and length
//! - `known_actions` / `is_known_action` allowlist checks
//!
//! ## Does not own
//! - Applying repairs (`repair`) or rebuilding indexes (`reindex`)
//! - The consistency checks the plan derives from (`verify`)
//!
//! ## Boundary
//!
//! Changes to plan composition or the allowlist should normally update tests
//! here. Changes to check logic should not require changes here.
//!
//! Keep this module focused on doctor planning behavior.

use crate::doctor::{
    ACTION_DROP_ORPHAN_ASSOC_EDGE, ACTION_REBUILD_DENSE_INDEX, ACTION_REBUILD_LEXICAL_INDEX,
    DoctorArgs, PlanRegistry, doctor_scoped, doctor_whole_store, is_known_action, known_actions,
};
use crate::finding::codes;
use clio_store::{AssocEdgeRecord, AssocKind, SqliteStore, Store};
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem};
use serde_json::json;
use std::sync::Arc;

const TS: &str = "2026-01-01T00:00:00Z";

fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(clio_store::LocalDevKms::new())).expect("in-memory store")
}

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn_1".to_owned()),
        snapshot: Some(json!({"error_code": "ECONNRESET"})),
        gist: Some("doctor fixture".to_owned()),
        valid_from: TS.to_owned(),
        valid_until: None,
        tx_from: TS.to_owned(),
        tx_until: None,
        created_at: TS.to_owned(),
        updated_at: TS.to_owned(),
        context: None,
    }
}

fn orphan_edge(bank: &str) -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: "orphan-1".to_owned(),
        bank_id: bank.to_owned(),
        source_id: "missing-a".to_owned(),
        target_id: "missing-b".to_owned(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some("r".to_owned()),
        weight: 0.5,
        last_updated_at: TS.to_owned(),
    }
}

fn plan_for(
    store: &SqliteStore,
    bank: Option<&str>,
    dry_run: bool,
) -> (PlanRegistry, crate::doctor::DoctorPlan) {
    let registry = PlanRegistry::new();
    let plan = doctor_whole_store(&DoctorArgs {
        store,
        index: store,
        bank,
        dry_run,
        timestamp: TS,
        plans: &registry,
    })
    .expect("doctor");
    (registry, plan)
}

#[test]
fn dry_run_plan_is_not_registered() {
    let store = store();
    let (registry, plan) = plan_for(&store, None, true);
    assert!(plan.dry_run);
    assert!(plan.ok);
    assert_eq!(plan.generated_at, TS);
    assert!(plan.actions.is_empty(), "{:?}", plan.actions);
    assert!(
        plan.notes
            .iter()
            .any(|n| n.contains(codes::MANIFEST_UNAVAILABLE))
    );
    assert!(registry.is_empty(), "{registry:?}");
    let registry_len = registry.len();
    assert_eq!(registry_len, 0);
    assert!(registry.get(&plan.plan_id).is_none());
}

#[test]
fn live_plan_is_registered_and_retrievable() {
    let store = store();
    let (registry, plan) = plan_for(&store, Some("b"), false);
    assert!(!plan.dry_run);
    assert_eq!(plan.bank.as_deref(), Some("b"));
    assert!(!registry.is_empty(), "planning registers a plan");
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.get(&plan.plan_id), Some(plan));
}

#[test]
fn orphan_edge_plan_includes_drop_action() {
    let store = store();
    store.put_assoc_edge(&orphan_edge("b")).unwrap();
    let (_registry, plan) = plan_for(&store, Some("b"), true);
    assert!(!plan.ok);
    assert!(
        plan.actions
            .iter()
            .any(|a| a.id == ACTION_DROP_ORPHAN_ASSOC_EDGE)
    );
}

#[test]
fn coverage_gap_plan_includes_rebuild_actions() {
    let store = store();
    store.create_memory_item(&item("i1", "b"), "actor").unwrap();
    let (_registry, plan) = plan_for(&store, Some("b"), true);
    let ids: Vec<&str> = plan.actions.iter().map(|a| a.id.as_str()).collect();
    assert!(ids.contains(&ACTION_REBUILD_DENSE_INDEX));
    assert!(ids.contains(&ACTION_REBUILD_LEXICAL_INDEX));
}

/// A bank-scoped plan sees only that bank. A gap in another bank must surface
/// as a note, so a clean plan is not read as a clean store.
#[test]
fn scoped_plan_notes_store_wide_findings_outside_the_bank() {
    let store = store();
    store.create_memory_item(&item("i1", "a"), "actor").unwrap();
    let (_registry, plan) = plan_for(&store, Some("b"), true);
    assert!(
        plan.notes
            .iter()
            .any(|note| note.contains("store-wide findings outside bank `b`")),
        "{:?}",
        plan.notes
    );
}

/// The note must fire even when the scoped bank carries the same error code as
/// another bank: the outside finding is real regardless.
#[test]
fn scoped_plan_notes_an_outside_bank_with_the_same_code() {
    let store = store();
    store.create_memory_item(&item("i1", "a"), "actor").unwrap();
    store.create_memory_item(&item("i2", "b"), "actor").unwrap();
    let (_registry, plan) = plan_for(&store, Some("b"), true);
    assert!(
        plan.notes
            .iter()
            .any(|note| note.contains("store-wide findings outside bank `b`")),
        "{:?}",
        plan.notes
    );
}

/// A bank-scoped plan reads no other bank: no store-wide note names another
/// bank, and no field carries its name or its findings.
#[test]
fn scoped_plan_never_reads_another_bank() {
    let store = store();
    store.create_memory_item(&item("i1", "a"), "actor").unwrap();
    store.put_assoc_edge(&orphan_edge("a")).unwrap();
    let registry = PlanRegistry::new();
    let plan = doctor_scoped(
        &DoctorArgs {
            store: &store,
            index: &store,
            bank: None,
            dry_run: true,
            timestamp: TS,
            plans: &registry,
        },
        "b",
    )
    .expect("doctor_scoped");
    // The named bank is the plan's scope whatever the args leave unsaid.
    assert_eq!(plan.bank.as_deref(), Some("b"));
    assert!(
        plan.notes
            .iter()
            .all(|note| !note.contains("store-wide findings outside")),
        "{:?}",
        plan.notes
    );
    let body = serde_json::to_string(&plan).expect("plan json");
    assert!(!body.contains("bank:a"), "{body}");
    assert!(!body.contains("ORPHAN_ASSOC_EDGE"), "{body}");
}

#[test]
fn allowlist_recognizes_exactly_the_known_actions() {
    assert_eq!(
        known_actions(),
        &[
            ACTION_DROP_ORPHAN_ASSOC_EDGE,
            ACTION_REBUILD_DENSE_INDEX,
            ACTION_REBUILD_LEXICAL_INDEX,
        ]
    );
    assert!(is_known_action(ACTION_DROP_ORPHAN_ASSOC_EDGE));
    assert!(is_known_action(ACTION_REBUILD_DENSE_INDEX));
    assert!(is_known_action(ACTION_REBUILD_LEXICAL_INDEX));
    assert!(!is_known_action("nope"));
}

#[test]
fn registry_allocates_unique_ids() {
    let registry = PlanRegistry::new();
    let first = registry.allocate_id();
    let second = registry.allocate_id();
    assert_ne!(first, second);
    assert_eq!(first, "plan-1");
    assert_eq!(second, "plan-2");
    assert!(registry.get(&first).is_none());
}
