// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Gated public writes: admission decides, the repository persists only on pass.

use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_admission::{
    AdmissionPolicy, Candidate, GateRequest, RankingEnv, SignalSource, VecSink, admit_preview,
    candidate_text, gated_create, gated_create_raw, signals_for,
};
use clio_types::{
    Actor, Bank, EpisodicType, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory,
};
use serde_json::json;
use std::sync::Arc;

use crate::test_unique::unique;

fn semantic_item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::TaskSpec),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: Some("turn_3".into()),
        snapshot: Some(json!({"task": "migrate the schema"})),
        gist: Some("task decision: must migrate the schema".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

fn gist_item(id: &str, bank: &str, gist: &str) -> MemoryItem {
    let mut item = semantic_item(id, bank);
    item.kind = ItemKind::Episodic;
    item.category = None;
    item.episodic_type = Some(EpisodicType::Gist);
    item.snapshot = Some(json!({"memo": "1"}));
    item.gist = Some(gist.into());
    item
}

fn ctx(bank: &str) -> OpContext {
    OpContext::new(Bank::new(bank.to_owned()), Actor::Agent)
}

fn gate_req<'a>(
    item: &'a MemoryItem,
    ctx: &'a OpContext,
    policy: AdmissionPolicy<'a>,
    sink: &'a VecSink,
) -> GateRequest<'a> {
    GateRequest {
        item,
        ctx,
        policy,
        sink,
        timestamp: "2026-09-17T00:00:00Z",
    }
}

/// Test signal source: scripted neighbors.
#[derive(Debug, Default)]
struct StubSource {
    mirror_item: bool,
}

impl SignalSource for StubSource {
    fn neighbor_texts(&self, item: &MemoryItem) -> Result<Vec<String>, clio_types::AmError> {
        if self.mirror_item {
            return Ok(vec![candidate_text(item)]);
        }
        Ok(vec![])
    }
}

fn memory_store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("in-memory store")
}

#[test]
fn gated_semantic_admit_persists_with_score() {
    let store = memory_store();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let bank = unique("gate-a");
    let id = unique("admit");
    let item = semantic_item(&id, &bank);
    let source = StubSource::default();

    let signals = signals_for(&item, &source).expect("signals");
    let preview = admit_preview(
        Candidate::Semantic(SemanticCategory::TaskSpec),
        &signals,
        &policy,
    )
    .expect("preview");
    assert!(preview.pass);
    // Preview alone persists nothing.
    assert!(store.get_memory_item(&bank, &id).expect("read").is_none());

    let decision = gated_create(
        &gate_req(&item, &ctx(&bank), policy.clone(), &sink),
        Candidate::Semantic(SemanticCategory::TaskSpec),
        &source,
        &mut |stored| store.create_memory_item(stored, "agent"),
    )
    .expect("gated");
    assert!(decision.pass);
    assert_eq!(decision, preview);
    let stored = store
        .get_memory_item(&bank, &id)
        .expect("read")
        .expect("admitted item persisted");
    assert_eq!(stored.admission_score, Some(decision.admission_score));
    assert_eq!(stored.snapshot, item.snapshot);
    assert_eq!(sink.events().len(), 1);
    assert_eq!(sink.events()[0].operation, "admit_accept");
    assert_eq!(stored.epistemic_kind, EpistemicKind::Fact);
    assert_eq!(sink.events()[0].candidate_key, "task_spec");
}

#[test]
fn gated_reject_persists_nothing() {
    let store = memory_store();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let bank = unique("gate-r");
    let id = unique("reject");
    let item = gist_item(&id, &bank, "hi");
    let source = StubSource { mirror_item: true };

    let decision = gated_create(
        &gate_req(&item, &ctx(&bank), policy.clone(), &sink),
        Candidate::Episodic(EpisodicType::Gist),
        &source,
        &mut |stored| store.create_memory_item(stored, "agent"),
    )
    .expect("reject, not error");
    assert!(!decision.pass);
    assert!(store.get_memory_item(&bank, &id).expect("read").is_none());
    assert_eq!(sink.events().len(), 1);
    assert_eq!(sink.events()[0].operation, "admit_reject");
    assert!(sink.events()[0].rejection_reason.is_some());
}

#[test]
fn double_admit_rescores_and_fails_on_duplicate() {
    let store = memory_store();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let bank = unique("gate-2x");
    let id = unique("twice");
    let item = semantic_item(&id, &bank);
    let source = StubSource::default();

    let first = gated_create(
        &gate_req(&item, &ctx(&bank), policy.clone(), &sink),
        Candidate::Semantic(SemanticCategory::TaskSpec),
        &source,
        &mut |stored| store.create_memory_item(stored, "agent"),
    )
    .expect("first admit");
    assert!(first.pass);

    // The retry still runs scoring (same passing decision, no bypass) and
    // then fails at the repository on the duplicate id: no silent second row.
    let second = gated_create(
        &gate_req(&item, &ctx(&bank), policy.clone(), &sink),
        Candidate::Semantic(SemanticCategory::TaskSpec),
        &source,
        &mut |stored| store.create_memory_item(stored, "agent"),
    )
    .expect_err("duplicate");
    assert!(second.message.contains("duplicate"));
    assert!(store.get_memory_item(&bank, &id).expect("read").is_some());
    assert_eq!(sink.events().len(), 1);
}

#[test]
fn episodic_admit_keeps_type_tag_without_forged_category() {
    let store = memory_store();
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let bank = unique("gate-e");
    let id = unique("epi");
    let mut item = gist_item(&id, &bank, "task decision: must rotate the logs");
    item.episodic_type = Some(EpisodicType::Task);
    let source = StubSource::default();

    let decision = gated_create_raw(
        &gate_req(&item, &ctx(&bank), policy.clone(), &sink),
        "episodic",
        "task",
        &source,
        &mut |stored| store.create_memory_item(stored, "agent"),
    )
    .expect("episodic admit");
    assert!(decision.pass);
    let stored = store
        .get_memory_item(&bank, &id)
        .expect("read")
        .expect("episodic item persisted");
    assert_eq!(stored.episodic_type, Some(EpisodicType::Task));
    assert_eq!(stored.category, None);
}

#[test]
fn postgres_gated_write_parity() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = crate::postgres::PostgresStore::open(&url, kms).expect("postgres open");
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let bank = unique("gate-pg");
    let admit_id = unique("pg-admit");
    let reject_id = unique("pg-reject");
    let admit_source = StubSource::default();
    let reject_source = StubSource { mirror_item: true };

    let decision = gated_create(
        &gate_req(
            &semantic_item(&admit_id, &bank),
            &ctx(&bank),
            policy.clone(),
            &sink,
        ),
        Candidate::Semantic(SemanticCategory::TaskSpec),
        &admit_source,
        &mut |stored| store.create_memory_item(stored, "agent"),
    )
    .expect("pg admit");
    assert!(decision.pass);
    assert!(
        store
            .get_memory_item(&bank, &admit_id)
            .expect("read")
            .is_some()
    );

    let decision = gated_create(
        &gate_req(
            &gist_item(&reject_id, &bank, "hi"),
            &ctx(&bank),
            policy,
            &sink,
        ),
        Candidate::Episodic(EpisodicType::Gist),
        &reject_source,
        &mut |stored| store.create_memory_item(stored, "agent"),
    )
    .expect("pg reject");
    assert!(!decision.pass);
    assert!(
        store
            .get_memory_item(&bank, &reject_id)
            .expect("read")
            .is_none()
    );
    assert_eq!(sink.events().len(), 2);
    assert!(sink.events().iter().all(|e| e.bank == bank));
}
