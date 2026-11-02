// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres task/failure/triple history reads.
//!
//! ## Owns
//! - The Postgres history read impls and row mappers
//!
//! ## Does not own
//! - The history write path or the SQLite twin
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

#![allow(clippy::assert_is_empty)]

use crate::history_store::HistoryStore;
use crate::postgres_belief::postgres_belief_context_tests::open_pg;
use crate::sqlite::sqlite_history_tests::{failure_draft, item, task_draft};
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{EpisodicType, EpistemicKind, TimeAxis, TripleRecord};

fn edge(bank: &str, item_id: &str) -> TripleRecord {
    TripleRecord {
        id: unique("pg-read-edge"),
        bank_id: bank.into(),
        item_id: item_id.into(),
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
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn postgres_history_reads_round_trip() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-read");

    let task_item = unique("pg-read-task-item");
    store
        .commit_task_upsert(
            &item(&task_item, &bank, EpisodicType::Task, false),
            &task_draft(&bank, &task_item),
            "agent",
        )
        .unwrap();
    let task = store.get_task_impl(&bank, "task-a").unwrap().expect("task");
    assert_eq!(task.definition, "fix the flaky login test");
    assert_eq!(task.steps, vec!["run tests".to_owned()]);
    assert_eq!(
        store.task_history_impl(&bank, "task-a", 10).unwrap().len(),
        1
    );
    assert_eq!(store.list_tasks_impl(&bank, 10).unwrap().len(), 1);
    assert!(store.get_task_impl(&bank, "missing").unwrap().is_none());

    let failure_item = unique("pg-read-failure-item");
    let stored = store
        .commit_failure_record(
            &item(&failure_item, &bank, EpisodicType::Failure, true),
            &failure_draft(&bank, &failure_item, 1),
            "agent",
        )
        .unwrap();
    let failure = store
        .get_failure_impl(&bank, &stored.id)
        .unwrap()
        .expect("failure");
    assert_eq!(failure.what_failed, "assertion error");
    assert_eq!(failure.lesson, "read the fixture");
    assert_eq!(
        store
            .failures_for_task_exact_impl(&bank, "task-a", 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.list_failures_impl(&bank, 10).unwrap().len(), 1);
    assert!(store.get_failure_impl(&bank, "missing").unwrap().is_none());

    let triple_item = unique("pg-read-triple-item");
    let mut carrier = item(&triple_item, &bank, EpisodicType::Triple, false);
    carrier.context = Some("carrier ctx".into());
    store
        .commit_triple_add(&carrier, &edge(&bank, &triple_item), false, "agent")
        .unwrap();
    let history = store
        .triple_history_impl(&bank, "Alice", "lives_in", 10)
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].object, "Boston");
    assert_eq!(history[0].context.as_deref(), Some("carrier ctx"));
    assert_eq!(
        store
            .triple_history_at_impl(
                &bank,
                "Alice",
                "lives_in",
                "2026-06-01T00:00:00Z",
                TimeAxis::Valid,
                10
            )
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .triple_history_at_impl(
                &bank,
                "Alice",
                "lives_in",
                "2026-05-01T00:00:00Z",
                TimeAxis::Valid,
                10
            )
            .unwrap()
            .is_empty()
    );
}
