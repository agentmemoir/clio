// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures used by the atomic batch
//! executor tests: store construction, item/triple builders, and the
//! all-or-nothing assertion exercised by both backends.
//!
//! ## Owns
//! - Test fixtures and builders for `BatchWrite` callers
//! - The `assert_all_or_nothing` persistence-contract assertion
//!
//! ## Does not own
//! - SQLite-specific batch tests (see `batch_tests`)
//! - Postgres parity tests (see `batch_pg_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::batch_model::{BatchStore, BatchWrite};
use crate::dek::{DekProvider, LocalDevKms};
use crate::model::{ItemInspectFilter, OpaqueItemRecord};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{
    BeliefObject, ConfidenceEntry, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem,
    PersonaPreference, SemanticCategory, SourceType, TripleRecord,
};
use std::sync::Arc;

use crate::test_unique::unique;

pub(crate) fn store() -> SqliteStore {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open_in_memory(kms).unwrap()
}

pub(crate) fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{id}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("schema").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({ "k": "v" })),
        gist: Some("gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

pub(crate) fn triple_item(id: &str, bank: &str) -> MemoryItem {
    let mut carrier = item(id, bank);
    carrier.kind = ItemKind::Episodic;
    carrier.category = None;
    carrier.episodic_type = Some(EpisodicType::Triple);
    carrier.subject_id = "subj".into();
    carrier.snapshot = Some(serde_json::json!({"subject": "s", "predicate": "p", "object": "o"}));
    carrier.gist = Some("s p o".into());
    item_shape_ok(&carrier);
    carrier
}

/// The batch executor must receive a valid carrier (same checks as the store).
pub(crate) fn item_shape_ok(item: &MemoryItem) {
    item.validate().unwrap();
}

pub(crate) fn triple(bank: &str, item_id: &str) -> TripleRecord {
    TripleRecord {
        context: None,
        id: format!("edge-{item_id}"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        subject: "s".into(),
        predicate: "p".into(),
        object: "o".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: None,
    }
}

pub(crate) fn assert_all_or_nothing(store: &(impl BatchStore + Store)) {
    // Three valid creates commit together.
    let bank = unique("b");
    let ids: Vec<String> = (0..3).map(|i| format!("itm-{i}-{}", unique("x"))).collect();
    let writes: Vec<BatchWrite> = ids
        .iter()
        .map(|id| BatchWrite::CreateItem {
            item: item(id, &bank),
            actor: "agent".into(),
        })
        .collect();
    store.apply_batch(&writes).unwrap();
    for id in &ids {
        assert!(store.get_memory_item(&bank, id).unwrap().is_some());
    }

    // The third write fails on a duplicate id → zero rows persist.
    let bank2 = unique("b");
    let dup = unique("dup");
    let writes = vec![
        BatchWrite::CreateItem {
            item: item("a", &bank2),
            actor: "agent".into(),
        },
        BatchWrite::CreateItem {
            item: item("b", &bank2),
            actor: "agent".into(),
        },
        BatchWrite::CreateItem {
            item: item(&dup, &bank2), // later re-created → duplicate id
            actor: "agent".into(),
        },
        BatchWrite::CreateItem {
            item: item(&dup, &bank2),
            actor: "agent".into(),
        },
    ];
    let err = store.apply_batch(&writes).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let mut filter = ItemInspectFilter::new(&bank2);
    filter.include_discarded = true;
    assert_eq!(store.count_items(&filter).unwrap(), 0);
}

pub(crate) fn record_shaped_writes(bank: &str) -> Vec<BatchWrite> {
    use clio_types::{FailureRecord, PersonaStableEntry, TaskRecord};
    let carrier_id = format!("carrier-{}", unique("c"));
    let failure_carrier_id = format!("{carrier_id}-f");
    let mut carrier = item(&carrier_id, bank);
    carrier.kind = ItemKind::Episodic;
    carrier.category = None;
    carrier.episodic_type = Some(EpisodicType::Task);
    let task = TaskRecord {
        context: None,
        id: format!("tr-{}", unique("t")),
        bank_id: bank.into(),
        item_id: carrier_id.clone(),
        task_id: "task-1".into(),
        subject_id: format!("subj-{bank}"),
        version: 1,
        definition: "fix the build".into(),
        steps: vec!["edit".into(), "test".into()],
        status: "in_progress".into(),
        evidence_ref: Some("turn_1".into()),
        admission_score: 0.0,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let failure = FailureRecord {
        context: None,
        id: format!("fr-{}", unique("f")),
        bank_id: bank.into(),
        item_id: failure_carrier_id.clone(),
        task_id: "task-1".into(),
        subject_id: format!("subj-{bank}"),
        attempt_n: 1,
        what_failed: "compile error".into(),
        lesson: "run cargo check first".into(),
        lesson_tokens: 5,
        evidence_ref: Some("trace_1".into()),
        admission_score: 0.0,
        epistemic_kind: EpistemicKind::Belief,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let entry = PersonaStableEntry {
        context: None,
        bank_id: bank.into(),
        key: "communication_style".into(),
        value: "concise".into(),
        confidence: Some(0.8),
        source: "explicit".into(),
        source_ref: Some("turn_1".into()),
        admission_score: 0.8,
        as_of: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
    };
    vec![
        BatchWrite::CommitTaskUpsert {
            item: carrier.clone(),
            record: task.clone(),
            actor: "agent".into(),
        },
        BatchWrite::CommitFailureRecord {
            item: {
                let mut c = carrier.clone();
                c.id = failure_carrier_id.clone();
                c.subject_id = format!("subj-{bank}-f");
                c.episodic_type = Some(EpisodicType::Failure);
                c.epistemic_kind = EpistemicKind::Belief;
                c.source_type = Some(clio_types::SourceType::AgentInferred);
                c.snapshot = Some(serde_json::json!({
                    "what_failed": "compile error",
                    "lesson": "run cargo check first",
                    "epistemic_kind": "belief"
                }));
                c
            },
            record: failure.clone(),
            actor: "agent".into(),
        },
        BatchWrite::PutStableEntry {
            entry,
            actor: "agent".into(),
        },
    ]
}

/// The import-facing batch variants: forced item overwrite, opaque restore,
/// preference upsert, gated belief create, and confidence append.
pub(crate) fn extended_writes(bank: &str) -> Vec<BatchWrite> {
    let upd_id = unique("upd");
    let opaque_id = unique("opq");
    let belief_id = unique("bel");
    let e1 = ConfidenceEntry::new("2026-01-01", 0.7, "e1", SourceType::UserStated).unwrap();
    let e2 = ConfidenceEntry::new("2026-02-01", 0.8, "e2", SourceType::UserStated).unwrap();
    vec![
        BatchWrite::CreateItem {
            item: item(&upd_id, bank),
            actor: "agent".into(),
        },
        BatchWrite::UpsertMemoryItem {
            item: {
                let mut it = item(&upd_id, bank);
                it.gist = Some("updated gist".into());
                it
            },
            actor: "agent".into(),
        },
        BatchWrite::PutOpaqueItem {
            item: OpaqueItemRecord {
                id: opaque_id,
                bank_id: bank.into(),
                subject_id: format!("subj-{bank}"),
                kind: "semantic".into(),
                category: Some("schema".into()),
                episodic_type: None,
                epistemic_kind: "fact".into(),
                source_type: None,
                confidence: Some(0.9),
                admission_score: Some(0.8),
                source_ref: None,
                valid_from: "2026-01-01T00:00:00Z".into(),
                valid_until: None,
                tx_from: "2026-01-01T00:00:00Z".into(),
                tx_until: None,
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
                content_cipher: "opaque-cipher".into(),
            },
            actor: "agent".into(),
        },
        BatchWrite::PutPreferenceState {
            preference: PersonaPreference {
                context: None,
                bank_id: bank.into(),
                key: "brevity_affinity".into(),
                value: 0.7,
                trend: 0.6,
                confidence: None,
                source: "explicit".into(),
                source_ref: None,
                admission_score: 0.7,
                as_of: "2026-01-01T00:00:00Z".into(),
            },
            actor: "agent".into(),
        },
        BatchWrite::CreateBelief {
            belief: BeliefObject {
                context: None,
                context_as_of: None,
                id: belief_id.clone(),
                bank_id: bank.into(),
                subject_id: format!("subj-{bank}"),
                proposition: "User must use the deploy checklist task".into(),
                confidence_history: vec![e1],
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
            actor: "agent".into(),
        },
        BatchWrite::AppendBeliefConfidence {
            bank_id: bank.into(),
            belief_id,
            entry: e2,
            actor: "agent".into(),
        },
    ]
}
