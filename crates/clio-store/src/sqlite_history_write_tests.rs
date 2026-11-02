// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite task/failure history write bodies.
//!
//! ## Owns
//! - `commit_task_upsert_impl`, `commit_failure_record_impl`, and their
//!   carrier validation
//!
//! ## Does not own
//! - The history read bodies (`sqlite_history_read`) or the sync feeds
//!   (`sqlite_sync_history`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::history_store::HistoryStore;
use crate::sqlite::sqlite_history_tests::{failure_draft, item, store, task_draft};
use crate::test_unique::unique;
use clio_types::{EpisodicType, ErrorCode};

#[test]
#[allow(clippy::too_many_lines)]
fn task_and_failure_writes_round_trip() {
    let store = store();
    let bank = unique("sqlite-write");

    let first_item = unique("sqlite-write-task-item");
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
    assert_eq!(first.created_at, "2026-06-01T00:00:00Z");

    let second_item = unique("sqlite-write-task-item");
    let second = store
        .commit_task_upsert_impl(
            &item(&second_item, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &second_item),
            "agent",
        )
        .unwrap();
    assert_eq!(second.version, 2);
    let latest = store.get_task(&bank, "task-a").unwrap().expect("latest");
    assert_eq!(latest.version, 2);
    assert_eq!(store.task_history(&bank, "task-a", 10).unwrap().len(), 2);

    let failure_item = unique("sqlite-write-failure-item");
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
        .expect("readable");
    assert_eq!(read.what_failed, "assertion error");
    assert_eq!(read.lesson, "read the fixture");
    assert_eq!(read.lesson_tokens, 3);
    assert_eq!(read.attempt_n, 1);
}

#[test]
fn duplicate_carrier_and_mismatched_draft_fail_closed() {
    let store = store();
    let bank = unique("sqlite-write-bad");

    let carrier = unique("sqlite-write-dup-item");
    store
        .commit_task_upsert_impl(
            &item(&carrier, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &carrier),
            "agent",
        )
        .unwrap();
    let duplicate = store
        .commit_task_upsert_impl(
            &item(&carrier, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &carrier),
            "agent",
        )
        .unwrap_err();
    assert_eq!(duplicate.code, ErrorCode::InvalidArgument);

    let wrong_type = unique("sqlite-write-wrong-item");
    let mismatch = store
        .commit_task_upsert_impl(
            &item(&wrong_type, &bank, EpisodicType::Failure, false),
            &task_draft(&bank, &wrong_type),
            "agent",
        )
        .unwrap_err();
    assert_eq!(mismatch.code, ErrorCode::InvalidArgument);

    let failure_carrier = unique("sqlite-write-wrong-failure");
    let failure_mismatch = store
        .commit_failure_record_impl(
            &item(&failure_carrier, &bank, EpisodicType::Task, true),
            &failure_draft(&bank, &failure_carrier, 2),
            "agent",
        )
        .unwrap_err();
    assert_eq!(failure_mismatch.code, ErrorCode::InvalidArgument);
}
