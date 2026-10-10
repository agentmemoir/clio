// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite history contract coverage:
//! task/failure/triple read-write, point-in-time reads, and bank isolation.
//!
//! ## Owns
//! - SQLite `HistoryStore` contract assertions and cross-bank rejection
//!
//! ## Does not own
//! - History context round trips, erased-subject context reads, and
//!   direct-write record-context validation (`sqlite_history_context_tests`)
//! - Postgres history coverage (`pg_history_tests`) or tool-level semantics
//!   (`clio-history`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::dek::{DekProvider, LocalDevKms};
use crate::history_store::HistoryStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, ErrorCode, FailureRecord, ItemKind, MemoryItem, TaskRecord,
    TimeAxis, TripleRecord,
};
use std::sync::Arc;

use crate::test_unique::unique;

pub(crate) fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

pub(crate) fn item(id: &str, bank: &str, episodic: EpisodicType, belief: bool) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subject-1".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(episodic),
        epistemic_kind: if belief {
            EpistemicKind::Belief
        } else {
            EpistemicKind::Fact
        },
        source_type: belief.then_some(clio_types::SourceType::AgentInferred),
        confidence: None,
        admission_score: Some(0.7),
        source_ref: None,
        snapshot: Some(serde_json::json!({"task_id": "task-a"})),
        gist: Some("fix the flaky login test".into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: None,
    }
}

pub(crate) fn task_draft(bank: &str, item_id: &str) -> TaskRecord {
    TaskRecord {
        id: unique("sqlite-task"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        task_id: "task-a".into(),
        subject_id: "subject-1".into(),
        version: 1,
        definition: "fix the flaky login test".into(),
        steps: vec!["run tests".into()],
        status: "in_progress".into(),
        evidence_ref: Some("turn_1".into()),
        context: None,
        admission_score: 0.7,
        created_at: "2026-06-01T00:00:00Z".into(),
    }
}

pub(crate) fn failure_draft(bank: &str, item_id: &str, attempt_n: i64) -> FailureRecord {
    FailureRecord {
        id: unique("sqlite-failure"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        task_id: "task-a".into(),
        subject_id: "subject-1".into(),
        attempt_n,
        what_failed: "assertion error".into(),
        lesson: "read the fixture".into(),
        lesson_tokens: 3,
        evidence_ref: Some("trace_1".into()),
        context: None,
        admission_score: 0.7,
        epistemic_kind: EpistemicKind::Belief,
        created_at: "2026-06-01T00:00:00Z".into(),
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn sqlite_history_contract() {
    let store = store();
    let bank = unique("sqlite-history");

    let first = unique("sqlite-task-item");
    let a = store
        .commit_task_upsert(
            &item(&first, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &first),
            "a",
        )
        .unwrap();
    assert_eq!(a.version, 1);
    let second = unique("sqlite-task-item");
    let b = store
        .commit_task_upsert(
            &item(&second, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &second),
            "a",
        )
        .unwrap();
    assert_eq!(b.version, 2);
    let latest = store.get_task(&bank, "task-a").unwrap().expect("latest");
    assert_eq!(latest.definition, "fix the flaky login test");
    assert_eq!(latest.steps, vec!["run tests".to_owned()]);
    assert!(store.get_task(&bank, "missing").unwrap().is_none());
    assert_eq!(store.task_history(&bank, "task-a", 10).unwrap().len(), 2);
    assert_eq!(store.list_tasks(&bank, 10).unwrap().len(), 1);

    let f_item = unique("sqlite-failure-item");
    store
        .commit_failure_record(
            &item(&f_item, &bank, EpisodicType::Failure, true),
            &failure_draft(&bank, &f_item, 1),
            "a",
        )
        .unwrap();
    let history = store.failures_for_task_exact(&bank, "task-a", 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].lesson, "read the fixture");
    let read = store
        .get_failure(&bank, &history[0].id)
        .unwrap()
        .expect("readable");
    assert_eq!(read.attempt_n, 1);
    assert!(store.get_failure(&bank, "missing").unwrap().is_none());
    assert_eq!(store.list_failures(&bank, 10).unwrap().len(), 1);

    let dup_item = unique("sqlite-failure-item");
    let dup = store.commit_failure_record(
        &item(&dup_item, &bank, EpisodicType::Failure, true),
        &failure_draft(&bank, &dup_item, 1),
        "a",
    );
    assert_eq!(dup.unwrap_err().code, ErrorCode::InvalidArgument);

    // Reusing a carrier item id trips the items-insert error mapper.
    let dup_carrier = store.commit_task_upsert(
        &item(&first, &bank, EpisodicType::Task, false),
        &task_draft(&bank, &first),
        "a",
    );
    assert_eq!(dup_carrier.unwrap_err().code, ErrorCode::InvalidArgument);

    // Carrier/draft mismatch and wrong episodic tag are rejected before write.
    let bad_item = unique("sqlite-bad-item");
    assert_eq!(
        store
            .commit_task_upsert(
                &item(&bad_item, &bank, EpisodicType::Failure, false),
                &task_draft(&bank, &bad_item),
                "a",
            )
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    let bad_failure = unique("sqlite-bad-failure");
    assert_eq!(
        store
            .commit_failure_record(
                &item(&bad_failure, &bank, EpisodicType::Task, true),
                &failure_draft(&bank, &bad_failure, 2),
                "a",
            )
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );

    let triple_item_id = unique("sqlite-triple-item");
    let edge = TripleRecord {
        id: unique("sqlite-edge"),
        bank_id: bank.clone(),
        item_id: triple_item_id.clone(),
        subject: "Alice".into(),
        predicate: "lives_in".into(),
        object: "Boston".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: None,
    };
    store
        .commit_triple_add(
            &item(&triple_item_id, &bank, EpisodicType::Triple, false),
            &edge,
            true,
            "a",
        )
        .unwrap();
    assert_eq!(
        store
            .triple_history(&bank, "Alice", "lives_in", 10)
            .unwrap()
            .len(),
        1
    );
    let unknown_predicate = store.triple_history(&bank, "Alice", "unknown", 10).unwrap();
    assert!(unknown_predicate.is_empty(), "{unknown_predicate:?}");

    // Point-in-time read applies the axis predicate before LIMIT.
    let at = store
        .triple_history_at(
            &bank,
            "Alice",
            "lives_in",
            "2026-06-01T00:00:00Z",
            TimeAxis::Valid,
            10,
        )
        .unwrap();
    assert_eq!(at.len(), 1);
    let valid_at_cutoff = store
        .triple_history_at(
            &bank,
            "Alice",
            "lives_in",
            "2026-05-01T00:00:00Z",
            TimeAxis::Valid,
            10,
        )
        .unwrap();
    assert!(valid_at_cutoff.is_empty(), "{valid_at_cutoff:?}");
}

#[test]
fn history_reads_are_bank_isolated() {
    let store = store();
    let bank_a = unique("iso-a");
    let bank_b = unique("iso-b");

    let task_item_id = unique("iso-task-item");
    store
        .commit_task_upsert(
            &item(&task_item_id, &bank_a, EpisodicType::Task, false),
            &task_draft(&bank_a, &task_item_id),
            "a",
        )
        .unwrap();
    let failure_item_id = unique("iso-failure-item");
    let failure = store
        .commit_failure_record(
            &item(&failure_item_id, &bank_a, EpisodicType::Failure, true),
            &failure_draft(&bank_a, &failure_item_id, 1),
            "a",
        )
        .unwrap();

    // Bank B sees none of bank A's task/failure history.
    assert!(store.get_task(&bank_b, "task-a").unwrap().is_none());
    assert!(store.get_failure(&bank_b, &failure.id).unwrap().is_none());
    let bank_b_failures = store.list_failures(&bank_b, 10).unwrap();
    assert!(bank_b_failures.is_empty(), "{bank_b_failures:?}");
    let bank_b_tasks = store.list_tasks(&bank_b, 10).unwrap();
    assert!(bank_b_tasks.is_empty(), "{bank_b_tasks:?}");
    let bank_b_exact_failures = store
        .failures_for_task_exact(&bank_b, "task-a", 10)
        .unwrap();
    assert!(
        bank_b_exact_failures.is_empty(),
        "{bank_b_exact_failures:?}"
    );
    let bank_b_task_history = store.task_history(&bank_b, "task-a", 10).unwrap();
    assert!(bank_b_task_history.is_empty(), "{bank_b_task_history:?}");

    // Bank A still sees its own records.
    assert!(store.get_task(&bank_a, "task-a").unwrap().is_some());
    assert!(store.get_failure(&bank_a, &failure.id).unwrap().is_some());
}
