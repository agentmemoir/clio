// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the predicate-level retrieve-domain wiring
//! and bounded-read tests for history carriers.
//!
//! ## Owns
//! - `item_in_domains` selection for task/failure carriers
//! - Bounded-limit and persona-domain exclusion assertions
//!
//! ## Does not own
//! - The end-to-end retrieve pipeline or compose tests
//!   (`retrieve_isolation_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::test_support::{failure_args, passing_env, query_args, setup, task_args};
use crate::{
    FailureRecordOutcome, TaskUpsertOutcome, failure_record, failures_for_task, task_upsert,
};
use clio_retrieve::item_in_domains;
use clio_store::Store;
use clio_types::{DEFAULT_HISTORY_LIMIT, HistoryPolicy, MAX_HISTORY_LIMIT, SearchDomain};

#[test]
fn t11_domains_select_only_their_history_type() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let steps = vec!["step".to_owned()];
    let TaskUpsertOutcome::Stored { record: task, .. } = task_upsert(&task_args(
        &setup,
        &env,
        "task-a",
        "fix flaky login",
        &steps,
        "done",
    ))
    .unwrap() else {
        panic!("expected stored task");
    };
    let FailureRecordOutcome::Stored {
        record: failure, ..
    } = failure_record(&failure_args(
        &setup, &env, &policy, "task-a", 1, "boom", "lesson",
    ))
    .unwrap()
    else {
        panic!("expected stored failure");
    };
    let bank = setup.ctx.bank.as_str();
    let task_item = setup
        .store
        .get_memory_item(bank, &task.item_id)
        .unwrap()
        .expect("task carrier");
    let failure_item = setup
        .store
        .get_memory_item(bank, &failure.item_id)
        .unwrap()
        .expect("failure carrier");

    assert!(item_in_domains(&task_item, &[SearchDomain::Task]));
    assert!(!item_in_domains(&task_item, &[SearchDomain::Failure]));
    assert!(item_in_domains(&failure_item, &[SearchDomain::Failure]));
    assert!(!item_in_domains(&failure_item, &[SearchDomain::Task]));
}

#[test]
fn t12_reads_are_bounded_no_full_log_api() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    assert_eq!(policy.resolve_limit(None).unwrap(), DEFAULT_HISTORY_LIMIT);
    assert_eq!(
        policy.resolve_limit(Some(10_000)).unwrap(),
        MAX_HISTORY_LIMIT
    );

    let steps = vec!["step".to_owned()];
    for attempt in 1..=3 {
        task_upsert(&task_args(
            &setup,
            &env,
            &format!("task-{attempt}"),
            "definition",
            &steps,
            "done",
        ))
        .unwrap();
        failure_record(&failure_args(
            &setup,
            &env,
            &policy,
            &format!("task-{attempt}"),
            1,
            "boom",
            "lesson",
        ))
        .unwrap();
    }
    let out =
        failures_for_task(&query_args(&setup, &policy, "definition", Some(usize::MAX))).unwrap();
    assert!(out.failures.len() <= MAX_HISTORY_LIMIT);
    // No API returns an unbounded transcript: the exact read is also capped.
    let exact =
        failures_for_task(&query_args(&setup, &policy, "task-1", Some(usize::MAX))).unwrap();
    assert!(exact.failures.len() <= MAX_HISTORY_LIMIT);
}

#[test]
fn t13_failures_never_appear_in_persona_domain() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let FailureRecordOutcome::Stored {
        record: failure, ..
    } = failure_record(&failure_args(
        &setup, &env, &policy, "task-a", 1, "boom", "lesson",
    ))
    .unwrap()
    else {
        panic!("expected stored failure");
    };
    let item = setup
        .store
        .get_memory_item(setup.ctx.bank.as_str(), &failure.item_id)
        .unwrap()
        .expect("failure carrier");
    assert!(!item_in_domains(&item, &[SearchDomain::Persona]));
    assert!(!item_in_domains(&item, &[SearchDomain::Semantic]));
    // The failure carrier is an episodic `failure` tag, never a persona item.
    assert_eq!(item.episodic_type, Some(clio_types::EpisodicType::Failure));
    assert!(item.category.is_none());
}
