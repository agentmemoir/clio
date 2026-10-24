// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the `repair` operator surface:
//! request refusal, confirmation gating, allowlisted execution, and targeted
//! re-verification.
//!
//! ## Owns
//! - `repair` refusal findings, action outcomes, and residual findings
//! - Local embedder doubles and the seeded SQLite fixture
//!
//! ## Does not own
//! - The repair allowlist or plan composition (`doctor`)
//! - The rebuild sweep (`reindex` / `clio_index::rebuild`)
//!
//! ## Boundary
//!
//! Changes to repair execution policy should normally update tests here.
//! Changes to the allowlist should not require changes here.
//!
//! Keep this module focused on repair behavior.

use crate::doctor::{
    ACTION_DROP_ORPHAN_ASSOC_EDGE, ACTION_REBUILD_DENSE_INDEX, ACTION_REBUILD_LEXICAL_INDEX,
    DoctorArgs, PlanRegistry, doctor_whole_store,
};
use crate::finding::codes;
use crate::repair::{ActionStatus, RepairArgs, RepairOutcome, repair};
use clio_index::Embedder;
use clio_store::{AssocEdgeRecord, AssocKind, EMBEDDING_DIMS, SqliteStore, Store};
use clio_types::{AmError, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem};
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
        gist: Some("repair fixture".to_owned()),
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

struct FakeEmbedder;

impl Embedder for FakeEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Ok(texts
            .iter()
            .map(|_| vec![0.0_f32; EMBEDDING_DIMS])
            .collect())
    }
    fn health(&self) -> Result<(), AmError> {
        Ok(())
    }
    fn model_id(&self) -> String {
        clio_store::DEFAULT_EMBEDDING_MODEL.to_owned()
    }
    fn dims(&self) -> usize {
        EMBEDDING_DIMS
    }
}

struct FailingEmbedder;

impl Embedder for FailingEmbedder {
    fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Err(AmError::new(
            ErrorCode::Internal,
            "sidecar down: postgres://ops:hunter2@db.host:5432/am",
        ))
    }
    fn health(&self) -> Result<(), AmError> {
        Err(AmError::new(ErrorCode::Internal, "sidecar down"))
    }
    fn model_id(&self) -> String {
        "fake/failing".to_owned()
    }
    fn dims(&self) -> usize {
        EMBEDDING_DIMS
    }
}

fn run(
    store: &SqliteStore,
    actions: &[String],
    bank: Option<&str>,
    confirm: bool,
    embedder: Option<&dyn Embedder>,
) -> RepairOutcome {
    let registry = PlanRegistry::new();
    repair(&RepairArgs {
        store,
        writer: store,
        index: store,
        embedder,
        plans: &registry,
        plan_id: None,
        actions,
        bank,
        confirm,
        timestamp: TS,
    })
    .expect("repair")
}

fn action(id: &str) -> Vec<String> {
    vec![id.to_owned()]
}

#[test]
fn no_plan_and_no_actions_is_refused() {
    let store = store();
    let outcome = run(&store, &[], None, true, None);
    assert!(!outcome.ok);
    assert!(!outcome.confirmed);
    assert!(outcome.results.is_empty(), "{:?}", outcome.results);
    assert!(
        outcome
            .findings
            .iter()
            .any(|f| f.code == codes::UNKNOWN_REPAIR_ACTION)
    );
}

#[test]
fn missing_plan_id_is_refused() {
    let store = store();
    let registry = PlanRegistry::new();
    let outcome = repair(&RepairArgs {
        store: &store,
        writer: &store,
        index: &store,
        embedder: None,
        plans: &registry,
        plan_id: Some("missing"),
        actions: &[],
        bank: None,
        confirm: true,
        timestamp: TS,
    })
    .expect("repair");
    assert!(!outcome.ok);
    assert!(
        outcome
            .findings
            .iter()
            .any(|f| f.code == codes::PLAN_NOT_FOUND)
    );
}

#[test]
fn unknown_action_is_refused() {
    let store = store();
    let outcome = run(&store, &action("nope"), Some("b"), true, None);
    assert!(!outcome.ok);
    assert!(
        outcome
            .findings
            .iter()
            .any(|f| f.code == codes::UNKNOWN_REPAIR_ACTION)
    );
}

#[test]
fn known_action_without_confirm_is_refused() {
    let store = store();
    let outcome = run(
        &store,
        &action(ACTION_DROP_ORPHAN_ASSOC_EDGE),
        Some("b"),
        false,
        None,
    );
    assert!(!outcome.ok);
    assert!(!outcome.confirmed);
    assert!(outcome.results.is_empty(), "{:?}", outcome.results);
    assert!(
        outcome
            .findings
            .iter()
            .any(|f| f.code == codes::CONFIRM_REQUIRED)
    );
}

#[test]
fn confirmed_drop_orphan_repairs_and_leaves_no_residual() {
    let store = store();
    store.put_assoc_edge(&orphan_edge("b")).unwrap();
    let outcome = run(
        &store,
        &action(ACTION_DROP_ORPHAN_ASSOC_EDGE),
        Some("b"),
        true,
        None,
    );
    assert!(outcome.ok);
    assert!(outcome.confirmed);
    assert_eq!(outcome.results.len(), 1);
    assert_eq!(outcome.results[0].status, ActionStatus::Repaired);
    assert_eq!(outcome.results[0].affected, 1);
    assert!(
        outcome.residual_findings.is_empty(),
        "{:?}",
        outcome.residual_findings
    );
}

#[test]
fn confirmed_drop_orphan_on_clean_store_is_skipped() {
    let store = store();
    let outcome = run(
        &store,
        &action(ACTION_DROP_ORPHAN_ASSOC_EDGE),
        Some("b"),
        true,
        None,
    );
    assert!(outcome.ok);
    assert_eq!(outcome.results[0].status, ActionStatus::Skipped);
    assert_eq!(outcome.results[0].affected, 0);
}

#[test]
fn confirmed_lexical_rebuild_repairs_coverage_gap() {
    let store = store();
    store.create_memory_item(&item("i1", "b"), "actor").unwrap();
    let outcome = run(
        &store,
        &action(ACTION_REBUILD_LEXICAL_INDEX),
        Some("b"),
        true,
        None,
    );
    assert!(outcome.ok);
    assert_eq!(outcome.results[0].status, ActionStatus::Repaired);
    assert_eq!(outcome.results[0].affected, 1);
    assert!(
        outcome.residual_findings.is_empty(),
        "{:?}",
        outcome.residual_findings
    );
}

#[test]
fn confirmed_dense_rebuild_with_embedder_repairs() {
    let store = store();
    store.create_memory_item(&item("i1", "b"), "actor").unwrap();
    let embedder = FakeEmbedder;
    let outcome = run(
        &store,
        &action(ACTION_REBUILD_DENSE_INDEX),
        Some("b"),
        true,
        Some(&embedder),
    );
    assert!(outcome.ok);
    assert_eq!(outcome.results[0].status, ActionStatus::Repaired);
    assert!(
        outcome.residual_findings.is_empty(),
        "{:?}",
        outcome.residual_findings
    );
}

#[test]
fn confirmed_dense_rebuild_without_embedder_fails() {
    let store = store();
    store.create_memory_item(&item("i1", "b"), "actor").unwrap();
    let outcome = run(
        &store,
        &action(ACTION_REBUILD_DENSE_INDEX),
        Some("b"),
        true,
        None,
    );
    assert!(!outcome.ok);
    assert_eq!(outcome.results[0].status, ActionStatus::Failed);
    assert!(outcome.results[0].detail.contains("embedding sidecar"));
}

#[test]
fn confirmed_rebuild_with_failing_embedder_fails() {
    let store = store();
    store.create_memory_item(&item("i1", "b"), "actor").unwrap();
    let embedder = FailingEmbedder;
    let outcome = run(
        &store,
        &action(ACTION_REBUILD_DENSE_INDEX),
        Some("b"),
        true,
        Some(&embedder),
    );
    assert!(!outcome.ok);
    assert_eq!(outcome.results[0].status, ActionStatus::Failed);
    assert!(outcome.results[0].detail.contains("rebuild failed"));
    // Error-derived detail must not leak credentialed URLs.
    assert!(
        !outcome.results[0].detail.contains("hunter2"),
        "{:?}",
        outcome.results[0].detail
    );
    assert!(
        outcome.results[0]
            .detail
            .contains("postgres://***@db.host:5432/am")
    );
}

#[test]
fn action_without_bank_scope_fails() {
    let store = store();
    let outcome = run(
        &store,
        &action(ACTION_DROP_ORPHAN_ASSOC_EDGE),
        None,
        true,
        None,
    );
    assert!(!outcome.ok);
    assert_eq!(outcome.results[0].status, ActionStatus::Failed);
    assert!(outcome.results[0].detail.contains("bank scope"));
}

#[test]
fn empty_bank_scope_action_fails() {
    let store = store();
    let outcome = run(
        &store,
        &action(ACTION_DROP_ORPHAN_ASSOC_EDGE),
        Some(""),
        true,
        None,
    );
    assert!(!outcome.ok);
    assert_eq!(outcome.results[0].status, ActionStatus::Failed);
    assert!(
        outcome.results[0]
            .detail
            .contains("drop orphan edges failed")
    );
}

#[test]
fn refused_plan_falls_back_to_plan_bank_scope() {
    let store = store();
    store.put_assoc_edge(&orphan_edge("b")).unwrap();
    let registry = PlanRegistry::new();
    let plan = doctor_whole_store(&DoctorArgs {
        store: &store,
        index: &store,
        bank: Some("b"),
        dry_run: false,
        timestamp: TS,
        plans: &registry,
    })
    .expect("doctor");
    let outcome = repair(&RepairArgs {
        store: &store,
        writer: &store,
        index: &store,
        embedder: None,
        plans: &registry,
        plan_id: Some(&plan.plan_id),
        actions: &[],
        bank: None,
        confirm: false,
        timestamp: TS,
    })
    .expect("repair");
    assert!(!outcome.ok);
    assert!(!outcome.confirmed);
    assert_eq!(outcome.bank.as_deref(), Some("b"));
    assert!(
        outcome
            .findings
            .iter()
            .any(|f| f.code == codes::CONFIRM_REQUIRED)
    );
}

#[test]
fn doctor_plan_id_resolves_and_executes() {
    let store = store();
    store.put_assoc_edge(&orphan_edge("b")).unwrap();
    let registry = PlanRegistry::new();
    let plan = doctor_whole_store(&DoctorArgs {
        store: &store,
        index: &store,
        bank: Some("b"),
        dry_run: false,
        timestamp: TS,
        plans: &registry,
    })
    .expect("doctor");
    let outcome = repair(&RepairArgs {
        store: &store,
        writer: &store,
        index: &store,
        embedder: None,
        plans: &registry,
        plan_id: Some(&plan.plan_id),
        actions: &[],
        bank: None,
        confirm: true,
        timestamp: TS,
    })
    .expect("repair");
    assert!(outcome.ok);
    assert_eq!(outcome.plan_id.as_deref(), Some(plan.plan_id.as_str()));
    assert_eq!(outcome.bank.as_deref(), Some("b"));
    assert_eq!(outcome.results[0].status, ActionStatus::Repaired);
}
