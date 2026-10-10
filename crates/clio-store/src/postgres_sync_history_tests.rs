// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres history sync surface behind the
//! `SyncStore` impl: the `feed_tasks` / `feed_failures` change feed bodies and
//! the `task_current` / `task_put` / `failure_by_attempt` / `failure_put` /
//! `belief_correct_context` apply delegations, exercised on the shared Compose
//! `clio` database with real carrier/record rows.
//!
//! ## Owns
//! - Postgres parity coverage for the task/failure history sync primitives
//!
//! ## Does not own
//! - SQLite history sync bodies (`sqlite_sync_history`, covered via `clio-sync`)
//! - Journal / cursor / DLQ parity (`postgres_sync` `pg_sync_tests`)
//!
//! ## Boundary
//!
//! Tests here break when Postgres history sync SQL changes; they never change it.
//!
//! Keep this module focused on Postgres history sync primitives.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::sync_store::SyncStore;
use crate::{DekProvider, LocalDevKms, PostgresStore};
use clio_types::{EpisodicType, EpistemicKind, FailureRecord, ItemKind, MemoryItem, TaskRecord};
use std::sync::Arc;

/// Empty-watermark sentinel the client push cursor uses before any push.
const SENTINEL: &str = "1970-01-01T00:00:00Z";

fn open_pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("postgres")
}

use crate::test_unique::unique;

fn task_item(bank: &str, item_id: &str, context: Option<&str>) -> MemoryItem {
    MemoryItem {
        id: item_id.into(),
        bank_id: bank.into(),
        subject_id: "subject-1".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.7),
        source_ref: None,
        snapshot: Some(serde_json::json!({"task_id": "task-sync"})),
        gist: Some("fix the flaky login test".into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: context.map(str::to_owned),
    }
}

fn task_draft(bank: &str, item_id: &str, task_id: &str, context: Option<&str>) -> TaskRecord {
    TaskRecord {
        id: unique("pg-task"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        task_id: task_id.into(),
        subject_id: "subject-1".into(),
        version: 1,
        definition: "fix the flaky login test".into(),
        steps: vec!["run tests".into()],
        status: "in_progress".into(),
        evidence_ref: Some("turn_1".into()),
        context: context.map(str::to_owned),
        admission_score: 0.7,
        created_at: "2026-06-01T00:00:00Z".into(),
    }
}

fn failure_item(bank: &str, item_id: &str, context: Option<&str>) -> MemoryItem {
    MemoryItem {
        subject_id: "subject-1".into(),
        episodic_type: Some(EpisodicType::Failure),
        epistemic_kind: EpistemicKind::Belief,
        source_type: Some(clio_types::SourceType::AgentInferred),
        ..task_item(bank, item_id, context)
    }
}

fn failure_draft(
    bank: &str,
    item_id: &str,
    attempt_n: i64,
    context: Option<&str>,
) -> FailureRecord {
    FailureRecord {
        id: unique("pg-failure"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        task_id: "task-sync".into(),
        subject_id: "subject-1".into(),
        attempt_n,
        what_failed: "assertion error".into(),
        lesson: "read the fixture".into(),
        lesson_tokens: 3,
        evidence_ref: Some("trace_1".into()),
        context: context.map(str::to_owned),
        admission_score: 0.7,
        epistemic_kind: EpistemicKind::Belief,
        created_at: "2026-06-01T00:00:00Z".into(),
    }
}

/// The history apply primitives: `task_put` / `task_current`, `failure_put`,
/// `failure_by_attempt`, the empty-bank feed delegations, and the
/// `belief_correct_context` correction path all round-trip through Postgres.
#[test]
#[allow(clippy::too_many_lines)]
fn pg_sync_history_apply_primitives_roundtrip() {
    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();

    // Empty-bank feed delegations (no row, so no row mapping runs).
    let empty_tasks = SyncStore::feed_tasks(&s, &bank, SENTINEL, 200).expect("feed tasks");
    assert!(empty_tasks.is_empty(), "{empty_tasks:?}");
    let empty_failures = SyncStore::feed_failures(&s, &bank, SENTINEL, 200).expect("feed failures");
    assert!(empty_failures.is_empty(), "{empty_failures:?}");

    // Task apply write through the `SyncStore` delegation (`task_put`).
    let item_id = unique("pg-task-item");
    let stored = SyncStore::task_put(
        &s,
        &task_item(&bank, &item_id, Some("task ctx")),
        &task_draft(&bank, &item_id, "task-sync", Some("task ctx")),
        "sync-apply",
    )
    .expect("task put");
    assert_eq!(stored.version, 1);
    assert_eq!(stored.context.as_deref(), Some("task ctx"));

    // Task apply read (`task_current`).
    let current = SyncStore::task_current(&s, &bank, "task-sync")
        .expect("current")
        .expect("row");
    assert_eq!(current.id, stored.id);
    assert_eq!(current.definition, "fix the flaky login test");
    assert!(
        SyncStore::task_current(&s, &bank, "task-missing")
            .expect("current")
            .is_none()
    );

    // Failure apply write through the `SyncStore` delegation (`failure_put`).
    let failure_item_id = unique("pg-failure-item");
    let f_stored = SyncStore::failure_put(
        &s,
        &failure_item(&bank, &failure_item_id, Some("fail ctx")),
        &failure_draft(&bank, &failure_item_id, 3, Some("fail ctx")),
        "sync-apply",
    )
    .expect("failure put");
    assert_eq!(f_stored.attempt_n, 3);

    // Failure apply read (`failure_by_attempt`), carrier context joined. The
    // INT4 `attempt_n` column is bound at its own width, so this must succeed.
    let row = SyncStore::failure_by_attempt(&s, &bank, "task-sync", 3)
        .expect("failure by attempt")
        .expect("row");
    assert_eq!(row.id, f_stored.id);
    assert_eq!(row.what_failed, "assertion error");
    assert_eq!(row.context.as_deref(), Some("fail ctx"));
    assert!(
        SyncStore::failure_by_attempt(&s, &bank, "task-sync", 9)
            .expect("missing attempt")
            .is_none()
    );

    // Belief context correction through the `SyncStore` delegation: the get
    // path sees the new value and the change feed surfaces the correction.
    let belief = clio_types::BeliefObject {
        id: format!("bel-{}", unique("pg")),
        bank_id: bank.clone(),
        subject_id: "subject-1".into(),
        proposition: "user prefers terse replies".into(),
        context: Some("original".into()),
        context_as_of: None,
        confidence_history: vec![
            clio_types::ConfidenceEntry::new(
                "2026-01-01",
                0.7,
                "ev",
                clio_types::SourceType::UserStated,
            )
            .expect("entry"),
        ],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    SyncStore::belief_create(&s, &belief, "seed").expect("belief create");
    SyncStore::belief_correct_context(&s, &bank, &belief.id, "corrected", "test", "agent")
        .expect("correct");
    let got = SyncStore::belief_get(&s, &bank, &belief.id)
        .expect("get")
        .expect("row");
    assert_eq!(got.context.as_deref(), Some("corrected"));
    let feed = SyncStore::feed_beliefs(&s, &bank, SENTINEL, 200).expect("feed beliefs");
    assert!(
        feed.iter()
            .any(|(_, b)| b.id == belief.id && b.context.as_deref() == Some("corrected"))
    );
}

/// The task/failure change feeds return a committed row with its change stamp
/// and opened definition/steps/what_failed/lesson. The INT4 columns are read at
/// their own width, so both feeds must map a real row.
#[test]
#[allow(clippy::too_many_lines)]
fn pg_sync_history_feeds_map_committed_rows() {
    let _guard = lock_pg_test();

    let s = open_pg();
    let bank = unique("bank");
    let item_id = unique("pg-task-item");
    SyncStore::task_put(
        &s,
        &task_item(&bank, &item_id, Some("task ctx")),
        &task_draft(&bank, &item_id, "task-sync", Some("task ctx")),
        "sync-apply",
    )
    .expect("task put");
    let feed = SyncStore::feed_tasks(&s, &bank, SENTINEL, 200).expect("feed tasks");
    assert_eq!(feed.len(), 1);
    let (stamp, got) = &feed[0];
    assert_eq!(stamp, "2026-06-01T00:00:00Z");
    assert_eq!(got.task_id, "task-sync");
    assert_eq!(got.version, 1);
    assert_eq!(got.definition, "fix the flaky login test");
    assert_eq!(got.steps, vec!["run tests".to_string()]);
    assert_eq!(got.status, "in_progress");
    assert!(got.context.is_none(), "the feed resolves no carrier text");
    let future_tasks = SyncStore::feed_tasks(&s, &bank, "9999-01-01T00:00:00Z", 200).expect("feed");
    assert!(future_tasks.is_empty(), "{future_tasks:?}");

    // Failure feed on a fresh store.
    let s2 = open_pg();
    let bank2 = unique("bank");
    let item2 = unique("pg-failure-item");
    SyncStore::failure_put(
        &s2,
        &failure_item(&bank2, &item2, Some("fail ctx")),
        &failure_draft(&bank2, &item2, 3, Some("fail ctx")),
        "sync-apply",
    )
    .expect("failure put");
    let feed = SyncStore::feed_failures(&s2, &bank2, SENTINEL, 200).expect("feed failures");
    assert_eq!(feed.len(), 1);
    let (stamp, got) = &feed[0];
    assert_eq!(stamp, "2026-06-01T00:00:00Z");
    assert_eq!(got.task_id, "task-sync");
    assert_eq!(got.attempt_n, 3);
    assert_eq!(got.what_failed, "assertion error");
    assert_eq!(got.lesson, "read the fixture");
    assert_eq!(got.lesson_tokens, 3);
    assert_eq!(got.epistemic_kind, EpistemicKind::Belief);
    assert!(got.context.is_none());
    let future_failures =
        SyncStore::feed_failures(&s2, &bank2, "9999-01-01T00:00:00Z", 200).expect("feed");
    assert!(future_failures.is_empty(), "{future_failures:?}");
}
