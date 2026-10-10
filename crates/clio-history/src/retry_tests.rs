// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the retry-surfacing tests: exact id,
//! retitled id, free-text match, and the no-match case.
//!
//! ## Owns
//! - `prepare_retry` and `failures_for_task` surfacing behavior
//! - Retitle-fixture-driven similar-task assertions
//!
//! ## Does not own
//! - Lexical similarity math (`similar_tests`) or the failure write path
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::similar::RETITLE_FIXTURES;
use crate::test_support::{failure_args, passing_env, query_args, retry_args, setup, task_args};
use crate::{failure_record, failures_for_task, prepare_retry, task_upsert};
use clio_types::HistoryPolicy;

#[test]
fn t07_retry_same_task_id_surfaces_exact_failure() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let steps = vec!["run tests".to_owned()];
    task_upsert(&task_args(
        &setup,
        &env,
        "task-a",
        "fix flaky login",
        &steps,
        "done",
    ))
    .unwrap();
    failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "task-a",
        1,
        "assertion failed",
        "read the fixture",
    ))
    .unwrap();
    let out = prepare_retry(&retry_args(&setup, &policy, "task-a", Some(5))).unwrap();
    assert_eq!(out.mode, "exact");
    assert!(out.task.is_some());
    assert_eq!(out.failures.len(), 1);
    assert_eq!(out.failures[0].lesson, "read the fixture");
}

#[test]
fn t07b_retitled_task_id_still_surfaces_prior_lesson() {
    for fixture in RETITLE_FIXTURES {
        let setup = setup();
        let env = passing_env();
        let policy = HistoryPolicy::default();
        let steps = vec!["step".to_owned()];
        task_upsert(&task_args(
            &setup,
            &env,
            fixture.prior_task_id,
            fixture.definition,
            &steps,
            "failed",
        ))
        .unwrap();
        failure_record(&failure_args(
            &setup,
            &env,
            &policy,
            fixture.prior_task_id,
            1,
            "first attempt failed",
            "the prior lesson to reuse",
        ))
        .unwrap();
        // Retry under a different task_id with the same definition.
        task_upsert(&task_args(
            &setup,
            &env,
            fixture.retitled_task_id,
            fixture.definition,
            &steps,
            "in_progress",
        ))
        .unwrap();
        let out = prepare_retry(&retry_args(
            &setup,
            &policy,
            fixture.retitled_task_id,
            Some(5),
        ))
        .unwrap();
        assert_eq!(out.mode, "similar", "fixture {}", fixture.prior_task_id);
        assert!(
            out.failures
                .iter()
                .any(|f| f.lesson == "the prior lesson to reuse"),
            "retitled task must surface the prior lesson"
        );
        assert!(
            out.matched_task_ids
                .iter()
                .any(|id| id == fixture.prior_task_id)
        );
    }
}

#[test]
fn prepare_retry_none_when_no_prior() {
    let setup = setup();
    let policy = HistoryPolicy::default();
    let out = prepare_retry(&retry_args(&setup, &policy, "unknown-task", None)).unwrap();
    assert_eq!(out.mode, "none");
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert!(out.task.is_none());
}

#[test]
fn failures_for_task_free_text_matches_definition() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let steps = vec!["step".to_owned()];
    task_upsert(&task_args(
        &setup,
        &env,
        "optimize-search-query",
        "optimize the slow search query to avoid a full table scan",
        &steps,
        "done",
    ))
    .unwrap();
    failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "optimize-search-query",
        1,
        "scan too slow",
        "add an index on the search column",
    ))
    .unwrap();
    let out = failures_for_task(&query_args(
        &setup,
        &policy,
        "slow search query full table scan",
        None,
    ))
    .unwrap();
    assert_eq!(out.mode, "similar");
    assert_eq!(out.failures.len(), 1);
    assert!(
        out.matched_task_ids
            .contains(&"optimize-search-query".into())
    );

    let miss = failures_for_task(&query_args(
        &setup,
        &policy,
        "completely unrelated task",
        None,
    ))
    .unwrap();
    assert_eq!(miss.mode, "none");
}

#[test]
fn failures_for_task_retitled_id_uses_own_definition() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let steps = vec!["step".to_owned()];
    let definition = "harden session cookies with secure and httponly flags";
    task_upsert(&task_args(
        &setup, &env, "old-id", definition, &steps, "failed",
    ))
    .unwrap();
    failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "old-id",
        1,
        "cookie leaked",
        "set httponly",
    ))
    .unwrap();
    task_upsert(&task_args(
        &setup,
        &env,
        "new-id",
        definition,
        &steps,
        "in_progress",
    ))
    .unwrap();
    let out = failures_for_task(&query_args(&setup, &policy, "new-id", None)).unwrap();
    assert_eq!(out.mode, "similar");
    assert_eq!(out.failures.len(), 1);
}
