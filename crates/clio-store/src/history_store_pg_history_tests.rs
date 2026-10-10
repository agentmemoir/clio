// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres history contract coverage,
//! serialized under the shared test lock.
//!
//! ## Owns
//! - Postgres task/failure/triple history read-write assertions
//!
//! ## Does not own
//! - SQLite history coverage (`sqlite_history_tests`) or the `HistoryStore`
//!   contract itself
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::dek::{DekProvider, LocalDevKms};
use crate::history_store::HistoryStore;
use crate::postgres::PostgresStore;
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, ErrorCode, FailureRecord, ItemKind, MemoryItem, TaskRecord,
    TimeAxis, TripleRecord,
};
use std::sync::Arc;

use crate::test_unique::unique;

fn task_item(bank: &str, item_id: &str) -> MemoryItem {
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

fn task_draft(bank: &str, item_id: &str) -> TaskRecord {
    TaskRecord {
        id: unique("pg-task"),
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

fn failure_item(bank: &str, item_id: &str) -> MemoryItem {
    let mut item = task_item(bank, item_id);
    item.episodic_type = Some(EpisodicType::Failure);
    item.epistemic_kind = EpistemicKind::Belief;
    item.source_type = Some(clio_types::SourceType::AgentInferred);
    item
}

fn failure_draft(bank: &str, item_id: &str, attempt_n: i64) -> FailureRecord {
    FailureRecord {
        id: unique("pg-failure"),
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
fn postgres_history_contract() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = PostgresStore::open(url.as_str(), Arc::clone(&kms)).unwrap();
    let bank = unique("pg-history");

    let first_item = unique("pg-task-item");
    let first = store
        .commit_task_upsert(
            &task_item(&bank, &first_item),
            &task_draft(&bank, &first_item),
            "a",
        )
        .unwrap();
    assert_eq!(first.version, 1);
    let second_item = unique("pg-task-item");
    let second = store
        .commit_task_upsert(
            &task_item(&bank, &second_item),
            &task_draft(&bank, &second_item),
            "a",
        )
        .unwrap();
    assert_eq!(second.version, 2);
    let latest = store.get_task(&bank, "task-a").unwrap().expect("latest");
    assert_eq!(latest.version, 2);
    assert_eq!(latest.definition, "fix the flaky login test");
    assert!(store.get_task(&bank, "missing").unwrap().is_none());
    assert_eq!(store.task_history(&bank, "task-a", 10).unwrap().len(), 2);
    assert_eq!(store.list_tasks(&bank, 10).unwrap().len(), 1);

    // Reusing a carrier item id trips the items-insert error mapper.
    let dup_carrier = store.commit_task_upsert(
        &task_item(&bank, &first_item),
        &task_draft(&bank, &first_item),
        "a",
    );
    assert_eq!(dup_carrier.unwrap_err().code, ErrorCode::InvalidArgument);

    let item_id = unique("pg-failure-item");
    store
        .commit_failure_record(
            &failure_item(&bank, &item_id),
            &failure_draft(&bank, &item_id, 1),
            "a",
        )
        .unwrap();
    let history = store.failures_for_task_exact(&bank, "task-a", 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].lesson, "read the fixture");
    let read = store
        .get_failure(&bank, &history[0].id)
        .unwrap()
        .expect("failure readable");
    assert_eq!(read.attempt_n, 1);
    assert!(store.get_failure(&bank, "missing").unwrap().is_none());
    assert_eq!(store.list_failures(&bank, 10).unwrap().len(), 1);

    let dup_item = unique("pg-failure-item");
    let dup = store.commit_failure_record(
        &failure_item(&bank, &dup_item),
        &failure_draft(&bank, &dup_item, 1),
        "a",
    );
    assert_eq!(dup.unwrap_err().code, ErrorCode::InvalidArgument);

    let triple_item_id = unique("pg-triple-item");
    let mut triple_item = task_item(&bank, &triple_item_id);
    triple_item.episodic_type = Some(EpisodicType::Triple);
    let edge = TripleRecord {
        id: unique("pg-edge"),
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
        .commit_triple_add(&triple_item, &edge, true, "a")
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
    assert_eq!(
        store
            .triple_history_at(
                &bank,
                "Alice",
                "lives_in",
                "2026-06-01T00:00:00Z",
                TimeAxis::Valid,
                10,
            )
            .unwrap()
            .len(),
        1
    );
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

/// Seed one task-history row (carrier + draft) with an optional context.
fn seed_ctx_task(
    store: &PostgresStore,
    bank: &str,
    item_id: &str,
    task_id: &str,
    subject: &str,
    context: Option<&str>,
) {
    let mut carrier = task_item(bank, item_id);
    carrier.subject_id = subject.into();
    carrier.context = context.map(str::to_owned);
    let mut draft = task_draft(bank, item_id);
    draft.task_id = task_id.into();
    draft.subject_id = subject.into();
    draft.context = context.map(str::to_owned);
    store.commit_task_upsert(&carrier, &draft, "a").unwrap();
}

/// Seed one failure-history row and return the stored record.
fn seed_ctx_failure(
    store: &PostgresStore,
    bank: &str,
    item_id: &str,
    task_id: &str,
    subject: &str,
    context: Option<&str>,
) -> FailureRecord {
    let mut carrier = failure_item(bank, item_id);
    carrier.subject_id = subject.into();
    carrier.context = context.map(str::to_owned);
    let mut draft = failure_draft(bank, item_id, 1);
    draft.task_id = task_id.into();
    draft.subject_id = subject.into();
    draft.context = context.map(str::to_owned);
    store.commit_failure_record(&carrier, &draft, "a").unwrap()
}

/// Postgres parity for the SQLite context coverage: every domain read surfaces
/// the exact sealed value, a record without context omits the key, an
/// unreadable carrier body degrades to `None`, and a crypto-shredded subject
/// fails closed exactly as before.
#[test]
#[allow(clippy::too_many_lines)]
fn postgres_history_context_parity() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = PostgresStore::open(url.as_str(), Arc::clone(&kms)).unwrap();
    let bank = unique("pg-ctx");
    let subject = unique("pg-ctx-subject");
    let task_item = unique("pg-ctx-task-item");
    let failure_item_id = unique("pg-ctx-failure-item");

    seed_ctx_task(
        &store,
        &bank,
        &task_item,
        "task-ctx",
        &subject,
        Some("team chat"),
    );
    seed_ctx_failure(
        &store,
        &bank,
        &failure_item_id,
        "task-ctx",
        &subject,
        Some("attempt 1 of 3"),
    );

    let task_ctx: Option<String> = Some("team chat".into());
    assert_eq!(
        store.get_task(&bank, "task-ctx").unwrap().unwrap().context,
        task_ctx
    );
    assert_eq!(
        store.task_history(&bank, "task-ctx", 10).unwrap()[0].context,
        task_ctx
    );
    let listed = store.list_tasks(&bank, 10).unwrap();
    let listed_ctx = listed.iter().find(|t| t.task_id == "task-ctx").unwrap();
    assert_eq!(listed_ctx.context, task_ctx);
    let raw = store
        .inspect_content_ciphertext(&bank, &task_item)
        .unwrap()
        .expect("sealed carrier");
    assert!(!raw.contains("team chat"), "context must stay sealed");

    let fail_ctx: Option<String> = Some("attempt 1 of 3".into());
    let exact = store
        .failures_for_task_exact(&bank, "task-ctx", 10)
        .unwrap();
    assert_eq!(exact[0].context, fail_ctx);
    assert_eq!(
        store
            .get_failure(&bank, &exact[0].id)
            .unwrap()
            .unwrap()
            .context,
        fail_ctx
    );
    assert_eq!(store.list_failures(&bank, 10).unwrap()[0].context, fail_ctx);

    // A record written without context omits the key entirely.
    let bare_item = unique("pg-noctx-task-item");
    seed_ctx_task(&store, &bank, &bare_item, "task-a", &subject, None);
    let bare = store.get_task(&bank, "task-a").unwrap().unwrap();
    assert!(bare.context.is_none());
    let json = serde_json::to_string(&bare).unwrap();
    assert!(
        !json.contains("\"context\""),
        "absent context must not serialize"
    );

    // A purged (unreadable) carrier body keeps the record readable with no
    // context; the left join degrades to `None`, it never errors.
    store
        .test_execute(&format!(
            "UPDATE items SET content_ciphertext = NULL WHERE id = '{task_item}'"
        ))
        .unwrap();
    let purged = store.get_task(&bank, "task-ctx").unwrap().unwrap();
    assert!(purged.context.is_none());
    assert_eq!(purged.definition, "fix the flaky login test");
}

/// Postgres parity for the crypto-shredded subject: the DEK is destroyed, the
/// context extraction stays tolerant, and the record read fails closed as
/// before. Uses a unique subject so the shred cannot poison other tests.
#[test]
fn postgres_history_erased_subject_fails_closed_and_hides_context() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = PostgresStore::open(url.as_str(), Arc::clone(&kms)).unwrap();
    let bank = unique("pg-ctx-shred");
    let subject = unique("pg-shred-subject");
    let failure_item_id = unique("pg-shred-failure-item");

    seed_ctx_task(
        &store,
        &bank,
        &unique("pg-shred-task-item"),
        "task-a",
        &subject,
        Some("team chat"),
    );
    let stored_failure = seed_ctx_failure(
        &store,
        &bank,
        &failure_item_id,
        "task-a",
        &subject,
        Some("attempt 1 of 3"),
    );

    kms.destroy_dek(&subject).unwrap();
    let raw = store
        .inspect_content_ciphertext(&bank, &failure_item_id)
        .unwrap()
        .expect("sealed body still present");
    // Context extraction tolerates the unreadable body: no error, no value.
    assert_eq!(
        crate::item_persist::open_context(kms.as_ref(), Some(&raw)),
        None
    );
    // The record's own sealed columns still fail closed, exactly as before.
    assert_eq!(
        store.get_task(&bank, "task-a").unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    assert_eq!(
        store
            .get_failure(&bank, &stored_failure.id)
            .unwrap_err()
            .code,
        ErrorCode::ErasedSubject
    );
    assert!(!raw.contains("team chat"), "shred must hide the context");
}
