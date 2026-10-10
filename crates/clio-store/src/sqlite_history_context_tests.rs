// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite history context contract: sealed
//! context round trips through the record reads, tolerant degradation to
//! `None`, fail-closed reads after a crypto shred, and record-context
//! validation on the direct task/failure write paths.
//!
//! ## Owns
//! - Context surface/degradation and erased-subject fail-closed history reads
//! - Direct task/failure write record-context validation
//!
//! ## Does not own
//! - The core SQLite history read/write contract (`sqlite_history_tests`)
//! - Postgres history coverage (`pg_history_tests`) or audit-context telemetry
//!   (`history_audit_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the history context contract.

use crate::dek::{DekProvider, LocalDevKms};
use crate::history_store::HistoryStore;
use crate::item_persist::open_context;
use crate::sqlite::SqliteStore;
use crate::sqlite::sqlite_history_tests::{failure_draft, item, store, task_draft};
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{EpisodicType, ErrorCode, FailureRecord, MemoryItem};
use std::sync::Arc;

/// Seed helper for the context tests: carriers and drafts share one subject.
struct CtxSeeder<'a> {
    store: &'a SqliteStore,
    bank: &'a str,
    subject: &'a str,
}

impl<'a> CtxSeeder<'a> {
    fn new(store: &'a SqliteStore, bank: &'a str, subject: &'a str) -> Self {
        Self {
            store,
            bank,
            subject,
        }
    }

    fn carrier(&self, episodic: EpisodicType, item_id: &str, context: Option<&str>) -> MemoryItem {
        let mut carrier = item(
            item_id,
            self.bank,
            episodic,
            episodic == EpisodicType::Failure,
        );
        carrier.subject_id = self.subject.into();
        carrier.context = context.map(str::to_owned);
        carrier
    }

    fn task(&self, item_id: &str, task_id: &str, context: Option<&str>) {
        let carrier = self.carrier(EpisodicType::Task, item_id, context);
        let mut draft = task_draft(self.bank, item_id);
        draft.task_id = task_id.into();
        draft.subject_id = self.subject.into();
        draft.context = context.map(str::to_owned);
        self.store
            .commit_task_upsert(&carrier, &draft, "a")
            .unwrap();
    }

    fn failure(&self, item_id: &str, task_id: &str, context: Option<&str>) -> FailureRecord {
        let carrier = self.carrier(EpisodicType::Failure, item_id, context);
        let mut draft = failure_draft(self.bank, item_id, 1);
        draft.task_id = task_id.into();
        draft.subject_id = self.subject.into();
        draft.context = context.map(str::to_owned);
        self.store
            .commit_failure_record(&carrier, &draft, "a")
            .unwrap()
    }
}

/// Context round trip: every domain read surfaces the exact sealed value, a
/// record without context omits the key, and an unreadable carrier body
/// degrades to `None` instead of failing the read.
#[test]
fn sqlite_history_context_surfaces_and_degrades_to_none() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let (bank, subject) = (unique("sqlite-ctx"), unique("ctx-subject"));
    let task_item = unique("ctx-task-item");
    let failure_item = unique("ctx-failure-item");
    let seeder = CtxSeeder::new(&store, &bank, &subject);

    seeder.task(&task_item, "task-ctx", Some("team chat"));
    seeder.failure(&failure_item, "task-ctx", Some("attempt 1 of 3"));

    let task_ctx: Option<String> = Some("team chat".into());
    let latest = store.get_task(&bank, "task-ctx").unwrap().unwrap();
    assert_eq!(latest.context, task_ctx);
    let history = store.task_history(&bank, "task-ctx", 10).unwrap();
    assert_eq!(history[0].context, task_ctx);
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
    let read = store.get_failure(&bank, &exact[0].id).unwrap().unwrap();
    assert_eq!(read.context, fail_ctx);
    let all = store.list_failures(&bank, 10).unwrap();
    assert_eq!(all[0].context, fail_ctx);

    // A record written without context omits the key entirely.
    let bare_item = unique("noctx-task-item");
    seeder.task(&bare_item, "task-a", None);
    let bare = store.get_task(&bank, "task-a").unwrap().unwrap();
    assert!(bare.context.is_none());
    let json = serde_json::to_string(&bare).unwrap();
    assert!(!json.contains("\"context\""), "no context key");

    // A purged (unreadable) carrier body keeps the record readable with no
    // context; the left join degrades to `None`, it never errors.
    let sql = format!("UPDATE items SET content_ciphertext = NULL WHERE id = '{task_item}'");
    store.test_execute(&sql).unwrap();
    let purged = store.get_task(&bank, "task-ctx").unwrap().unwrap();
    assert!(purged.context.is_none());
    assert_eq!(purged.definition, "fix the flaky login test");
}

/// A crypto-shredded subject destroys the DEK: the sealed context becomes
/// unreadable, the record read refuses fail-closed as before, and context
/// extraction itself stays tolerant (no leak, no crash).
#[test]
fn sqlite_history_erased_subject_fails_closed_and_hides_context() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let (bank, subject) = (unique("sqlite-ctx-shred"), unique("shred-subject"));
    let failure_item = unique("shred-failure-item");
    let seeder = CtxSeeder::new(&store, &bank, &subject);

    seeder.task(&unique("shred-task-item"), "task-a", Some("team chat"));
    let stored = seeder.failure(&failure_item, "task-a", Some("attempt 1 of 3"));

    kms.destroy_dek(&subject).unwrap();
    let raw = store
        .inspect_content_ciphertext(&bank, &failure_item)
        .unwrap()
        .expect("sealed body still present");
    assert_eq!(open_context(kms.as_ref(), Some(&raw)), None);
    let task_err = store.get_task(&bank, "task-a").unwrap_err();
    assert_eq!(task_err.code, ErrorCode::ErasedSubject);
    let failure_err = store.get_failure(&bank, &stored.id).unwrap_err();
    assert_eq!(failure_err.code, ErrorCode::ErasedSubject);
    assert!(!raw.contains("team chat"), "shred must hide the context");
}

#[test]
fn task_and_failure_direct_writes_validate_record_context() {
    let store = store();
    let bank = unique("sqlite-ctx");
    let item_id = unique("sqlite-task-item");

    // An oversized record context fails closed before sealing/SQL.
    let mut carrier = item(&item_id, &bank, EpisodicType::Task, false);
    let mut draft = task_draft(&bank, &item_id);
    carrier.context = Some("x".repeat(4097));
    draft.context = Some("x".repeat(4097));
    let err = store
        .commit_task_upsert(&carrier, &draft, "a")
        .expect_err("oversized record context");
    assert_eq!(err.code, ErrorCode::OutOfRange, "{err}");

    // A record context the carrier does not store would be silently dropped by
    // the read path (context resolves from the carrier), so it fails closed.
    let carrier = item(&item_id, &bank, EpisodicType::Task, false);
    draft.context = Some("bounded note".into());
    let err = store
        .commit_task_upsert(&carrier, &draft, "a")
        .expect_err("divergent record context");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err}");
    assert!(
        store.list_tasks(&bank, 10).unwrap().is_empty(),
        "rejected attempts must not leave a task row"
    );

    // A consistent pair commits and the domain read surfaces the context.
    let mut carrier = item(&item_id, &bank, EpisodicType::Task, false);
    carrier.context = Some("bounded note".into());
    let stored = store.commit_task_upsert(&carrier, &draft, "a").unwrap();
    assert_eq!(stored.context.as_deref(), Some("bounded note"));
    let read = store.get_task(&bank, "task-a").unwrap().expect("task read");
    assert_eq!(read.context.as_deref(), Some("bounded note"));
    assert_eq!(store.list_tasks(&bank, 10).unwrap().len(), 1);

    // The failure record path enforces the same contract.
    let failure_item_id = unique("sqlite-failure-item");
    let mut f_item = item(&failure_item_id, &bank, EpisodicType::Failure, true);
    let mut f_draft = failure_draft(&bank, &failure_item_id, 1);
    f_item.context = Some("y".repeat(4097));
    f_draft.context = Some("y".repeat(4097));
    let err = store
        .commit_failure_record(&f_item, &f_draft, "a")
        .expect_err("oversized failure context");
    assert_eq!(err.code, ErrorCode::OutOfRange, "{err}");
    let failures = store.list_failures(&bank, 10).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}
