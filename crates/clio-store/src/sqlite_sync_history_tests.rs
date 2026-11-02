// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite task/failure sync change feeds.
//!
//! ## Owns
//! - `feed_tasks_sqlite`, `feed_failures_sqlite`, and
//!   `failure_by_attempt_sqlite`
//!
//! ## Does not own
//! - Item/assoc/triple/persona feeds (`sync_records`) or the apply path
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

#![allow(clippy::assert_is_empty)]

use crate::history_store::HistoryStore;
use crate::sqlite::sqlite_history_tests::{failure_draft, item, store, task_draft};
use crate::test_unique::unique;
use clio_types::EpisodicType;

const EPOCH: &str = "1970-01-01T00:00:00Z";
const LATER: &str = "9999-01-01T00:00:00Z";

#[test]
#[allow(clippy::too_many_lines)]
fn history_sync_feeds_open_task_and_failure_rows() {
    let store = store();
    let bank = unique("sqlite-sync-history");

    let task_item = unique("sqlite-sync-task-item");
    store
        .commit_task_upsert(
            &item(&task_item, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &task_item),
            "agent",
        )
        .unwrap();
    let tasks = super::feed_tasks_sqlite(&store, &bank, EPOCH, 10).unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(!tasks[0].0.is_empty());
    assert_eq!(tasks[0].1.definition, "fix the flaky login test");
    assert_eq!(tasks[0].1.steps, vec!["run tests".to_owned()]);
    assert_eq!(tasks[0].1.bank_id, bank);
    assert_eq!(tasks[0].1.context, None);
    assert!(
        super::feed_tasks_sqlite(&store, &bank, LATER, 10)
            .unwrap()
            .is_empty()
    );

    let failure_item = unique("sqlite-sync-failure-item");
    let mut carrier = item(&failure_item, &bank, EpisodicType::Failure, true);
    carrier.context = Some("failure ctx".into());
    let stored = store
        .commit_failure_record(&carrier, &failure_draft(&bank, &failure_item, 1), "agent")
        .unwrap();
    let failures = super::feed_failures_sqlite(&store, &bank, EPOCH, 10).unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].1.what_failed, "assertion error");
    assert_eq!(failures[0].1.lesson, "read the fixture");
    assert_eq!(
        failures[0].1.epistemic_kind,
        clio_types::EpistemicKind::Belief
    );
    assert_eq!(failures[0].1.context, None);
    assert!(
        super::feed_failures_sqlite(&store, &bank, LATER, 10)
            .unwrap()
            .is_empty()
    );

    let by_attempt = super::failure_by_attempt_sqlite(&store, &bank, "task-a", 1)
        .unwrap()
        .expect("failure by attempt");
    assert_eq!(by_attempt.id, stored.id);
    assert_eq!(by_attempt.context.as_deref(), Some("failure ctx"));
    assert!(
        super::failure_by_attempt_sqlite(&store, &bank, "task-a", 2)
            .unwrap()
            .is_none()
    );
}
