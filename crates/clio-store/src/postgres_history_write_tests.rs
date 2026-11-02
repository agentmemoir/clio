// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres task/failure history writes.
//!
//! ## Owns
//! - `commit_task_upsert_impl`, `commit_failure_record_impl`, and their
//!   transaction bodies
//!
//! ## Does not own
//! - The history read path or the SQLite twin
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::history_store::HistoryStore;
use crate::postgres_belief::postgres_belief_context_tests::open_pg;
use crate::sqlite::sqlite_history_tests::{failure_draft, item, task_draft};
use crate::test_unique::unique;
use clio_types::{EpisodicType, ErrorCode};

#[test]
#[allow(clippy::too_many_lines)]
fn postgres_history_writes_round_trip() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-write");

    let first_item = unique("pg-write-task-item");
    let first = store
        .commit_task_upsert_impl(
            &item(&first_item, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &first_item),
            "agent",
        )
        .unwrap();
    assert_eq!(first.version, 1);
    assert_eq!(first.definition, "fix the flaky login test");
    assert_eq!(first.steps, vec!["run tests".to_owned()]);
    assert_eq!(first.admission_score, 0.7);

    let second_item = unique("pg-write-task-item");
    let second = store
        .commit_task_upsert_impl(
            &item(&second_item, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &second_item),
            "agent",
        )
        .unwrap();
    assert_eq!(second.version, 2);
    assert_eq!(store.get_task(&bank, "task-a").unwrap().unwrap().version, 2);

    let failure_item = unique("pg-write-failure-item");
    let stored = store
        .commit_failure_record_impl(
            &item(&failure_item, &bank, EpisodicType::Failure, true),
            &failure_draft(&bank, &failure_item, 1),
            "agent",
        )
        .unwrap();
    let read = store
        .get_failure(&bank, &stored.id)
        .unwrap()
        .expect("failure");
    assert_eq!(read.what_failed, "assertion error");
    assert_eq!(read.lesson, "read the fixture");
    assert_eq!(read.attempt_n, 1);

    let duplicate = store
        .commit_task_upsert_impl(
            &item(&first_item, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &first_item),
            "agent",
        )
        .unwrap_err();
    assert_eq!(duplicate.code, ErrorCode::InvalidArgument);
}
