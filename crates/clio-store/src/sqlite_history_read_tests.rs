// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite task/failure history read bodies.
//!
//! ## Owns
//! - `get_task_impl`, `task_history_impl`, `list_tasks_impl`, the failure
//!   readers, and both row mappers
//!
//! ## Does not own
//! - The history write path (`sqlite_history_write`) or the triple readers
//!   (`sqlite_history_triple`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

#![allow(clippy::assert_is_empty)]

use crate::history_store::HistoryStore;
use crate::sqlite::sqlite_history_tests::{failure_draft, item, store, task_draft};
use crate::test_unique::unique;
use clio_types::{EpisodicType, EpistemicKind};

#[test]
#[allow(clippy::too_many_lines)]
fn task_and_failure_reads_round_trip() {
    let store = store();
    let bank = unique("sqlite-read");

    let task_item_id = unique("sqlite-read-task-item");
    store
        .commit_task_upsert(
            &item(&task_item_id, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &task_item_id),
            "agent",
        )
        .unwrap();

    let task = store
        .get_task_impl(&bank, "task-a")
        .unwrap()
        .expect("stored task");
    assert_eq!(task.definition, "fix the flaky login test");
    assert_eq!(task.steps, vec!["run tests".to_owned()]);
    assert_eq!(task.version, 1);
    assert_eq!(task.evidence_ref.as_deref(), Some("turn_1"));

    let history = store.task_history_impl(&bank, "task-a", 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, task.id);

    let listed = store.list_tasks_impl(&bank, 10).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(store.get_task_impl(&bank, "missing").unwrap().is_none());
    assert!(
        store
            .task_history_impl(&bank, "missing", 10)
            .unwrap()
            .is_empty()
    );

    let failure_item_id = unique("sqlite-read-failure-item");
    let stored_failure = store
        .commit_failure_record(
            &item(&failure_item_id, &bank, EpisodicType::Failure, true),
            &failure_draft(&bank, &failure_item_id, 1),
            "agent",
        )
        .unwrap();

    let failure = store
        .get_failure_impl(&bank, &stored_failure.id)
        .unwrap()
        .expect("stored failure");
    assert_eq!(failure.what_failed, "assertion error");
    assert_eq!(failure.lesson, "read the fixture");
    assert_eq!(failure.lesson_tokens, 3);
    assert_eq!(failure.epistemic_kind, EpistemicKind::Belief);

    let for_task = store
        .failures_for_task_exact_impl(&bank, "task-a", 10)
        .unwrap();
    assert_eq!(for_task.len(), 1);
    assert_eq!(for_task[0].id, failure.id);

    let all = store.list_failures_impl(&bank, 10).unwrap();
    assert_eq!(all.len(), 1);
    assert!(store.get_failure_impl(&bank, "missing").unwrap().is_none());
    assert!(
        store
            .list_failures_impl(&unique("other-bank"), 10)
            .unwrap()
            .is_empty()
    );
}
