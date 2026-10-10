// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `task_upsert` / `task_get` /
//! `task_history` tool tests.
//!
//! ## Owns
//! - Task upsert/get round-trip, history caps, and prior-anchor reporting
//! - Gate-reject and id/field validation paths
//!
//! ## Does not own
//! - Failure writes (`failure_tests`) or store-level history SQL
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::test_support::{T0, T1, get_args, history_args, passing_env, setup, task_args};
use crate::{TaskUpsertOutcome, task_get, task_history, task_upsert};
use clio_store::{HistoryStore, MemTreeNodeRecord, Store};
use clio_types::{ErrorCode, HistoryPolicy};

fn steps() -> Vec<String> {
    vec!["read the failing test".into(), "patch the fixture".into()]
}

#[test]
fn t01_upsert_then_get_roundtrip() {
    let setup = setup();
    let env = passing_env();
    let steps = steps();
    let out = task_upsert(&task_args(
        &setup,
        &env,
        "task-a",
        "fix flaky login",
        &steps,
        "in_progress",
    ))
    .unwrap();
    let TaskUpsertOutcome::Stored { record, decision } = out else {
        panic!("expected stored");
    };
    assert!(decision.pass);
    assert_eq!(record.task_id, "task-a");
    assert_eq!(record.version, 1);
    assert_eq!(record.definition, "fix flaky login");
    assert_eq!(record.steps, steps);
    assert!(record.admission_score >= 0.0);

    let got = task_get(&get_args(&setup, "task-a")).unwrap();
    let stored = got.record.expect("record present");
    assert_eq!(stored.id, record.id);
    assert_eq!(stored.version, 1);
    assert_eq!(stored.status, "in_progress");
    assert!(got.ancestor_gist.is_none());
}

#[test]
fn t02_task_history_caps_rows() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let steps = steps();
    for _ in 0..3 {
        task_upsert(&task_args(
            &setup,
            &env,
            "task-a",
            "fix flaky login",
            &steps,
            "in_progress",
        ))
        .unwrap();
    }
    let capped = task_history(&history_args(&setup, &policy, "task-a", Some(1))).unwrap();
    assert_eq!(capped.limit, 1);
    assert_eq!(capped.records.len(), 1);
    assert_eq!(capped.records[0].version, 3);

    let two = task_history(&history_args(&setup, &policy, "task-a", Some(2))).unwrap();
    assert_eq!(two.records.len(), 2);
    assert_eq!(
        two.records.iter().map(|r| r.version).collect::<Vec<_>>(),
        vec![3, 2]
    );

    let defaulted = task_history(&history_args(&setup, &policy, "task-a", None)).unwrap();
    assert_eq!(defaulted.limit, policy.default_limit);
    assert_eq!(defaulted.records.len(), 3);

    let zero = task_history(&history_args(&setup, &policy, "task-a", Some(0))).unwrap_err();
    assert_eq!(zero.code, ErrorCode::OutOfRange);

    let huge = task_history(&history_args(&setup, &policy, "task-a", Some(usize::MAX))).unwrap();
    assert_eq!(huge.limit, policy.max_limit);
    assert_eq!(huge.records.len(), 3);
}

#[test]
fn task_get_unknown_is_none_and_reports_ancestor() {
    let setup = setup();
    let unknown = task_get(&get_args(&setup, "nope")).unwrap();
    assert!(unknown.record.is_none());

    setup
        .store
        .upsert_memtree_node(&MemTreeNodeRecord {
            id: "node-1".into(),
            bank_id: setup.ctx.bank.as_str().into(),
            parent_id: None,
            item_id: None,
            depth: 1,
            interval_start: T0.into(),
            interval_end: T1.into(),
            child_ids: vec![],
            summary_gist: Some("fixed the login test twice".into()),
            dirty: false,
            generation: 1,
            refreshing: false,
            task_id: Some("task-a".into()),
            last_error: None,
            created_at: T0.into(),
            updated_at: T0.into(),
        })
        .unwrap();
    let got = task_get(&get_args(&setup, "task-a")).unwrap();
    assert_eq!(
        got.ancestor_gist.as_deref(),
        Some("fixed the login test twice")
    );
}

#[test]
fn upsert_rejected_by_gate_writes_nothing() {
    let setup = setup();
    let env = crate::test_support::failing_env();
    let steps = steps();
    let out = task_upsert(&task_args(
        &setup,
        &env,
        "task-a",
        "fix flaky login",
        &steps,
        "in_progress",
    ))
    .unwrap();
    assert!(matches!(out, TaskUpsertOutcome::Rejected { .. }));
    assert!(
        setup
            .store
            .get_task(setup.ctx.bank.as_str(), "task-a")
            .unwrap()
            .is_none()
    );
    assert_eq!(setup.sink.events()[0].outcome, "rejected");
}

#[test]
fn upsert_validates_ids() {
    let setup = setup();
    let env = passing_env();
    let steps = steps();
    let err = task_upsert(&task_args(&setup, &env, "", "def", &steps, "s")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = task_upsert(&task_args(&setup, &env, "t", "", &steps, "s")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = task_upsert(&task_args(&setup, &env, "t", "def", &steps, "")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn list_tasks_returns_latest_per_id() {
    let setup = setup();
    let env = passing_env();
    let steps = steps();
    task_upsert(&task_args(&setup, &env, "a", "task a", &steps, "done")).unwrap();
    task_upsert(&task_args(&setup, &env, "a", "task a v2", &steps, "done")).unwrap();
    task_upsert(&task_args(&setup, &env, "b", "task b", &steps, "done")).unwrap();
    let tasks = setup.store.list_tasks(setup.ctx.bank.as_str(), 10).unwrap();
    assert_eq!(tasks.len(), 2);
    let a = tasks.iter().find(|t| t.task_id == "a").expect("a");
    assert_eq!(a.version, 2);
    assert_eq!(a.definition, "task a v2");
}

#[test]
fn dry_run_scores_and_stages_without_persisting() {
    let s = crate::test_support::setup();
    let env = crate::test_support::passing_env();
    let steps = vec!["edit".to_owned()];
    let mut args = crate::test_support::task_args(
        &s,
        &env,
        "task-dry",
        "fix flaky login",
        &steps,
        "in_progress",
    );
    args.dry_run = true;
    match task_upsert(&args).unwrap() {
        TaskUpsertOutcome::DryRun {
            item,
            record,
            decision,
        } => {
            assert!(decision.pass);
            assert_eq!(item.id, record.item_id);
        }
        _ => panic!("expected dry-run outcome"),
    }
    assert!(
        s.store
            .get_task(s.ctx.bank.as_str(), "task-dry")
            .unwrap()
            .is_none()
    );
}

/// A supplied context is sealed into the carrier item and surfaced by the
/// domain reads; a record without context serializes no `context` key.
#[test]
fn context_is_sealed_into_the_carrier_and_surfaced() {
    let setup = setup();
    let env = passing_env();
    let steps = steps();
    let mut args = task_args(
        &setup,
        &env,
        "task-a",
        "fix flaky login",
        &steps,
        "in_progress",
    );
    args.context = Some("team chat");
    let TaskUpsertOutcome::Stored { record, .. } = task_upsert(&args).unwrap() else {
        panic!("expected stored");
    };
    assert_eq!(record.context.as_deref(), Some("team chat"));
    let got = task_get(&get_args(&setup, "task-a")).unwrap();
    assert_eq!(
        got.record.expect("record present").context.as_deref(),
        Some("team chat")
    );

    let out = task_upsert(&task_args(&setup, &env, "task-bare", "def", &steps, "s")).unwrap();
    let TaskUpsertOutcome::Stored { record, .. } = out else {
        panic!("expected stored");
    };
    assert!(record.context.is_none());
    assert!(
        !serde_json::to_string(&record)
            .unwrap()
            .contains("\"context\""),
        "absent context must not serialize"
    );
}

/// An invalid context fails closed before the gate: no admission decision is
/// emitted and nothing reaches the store.
#[test]
fn invalid_context_fails_closed_before_the_gate() {
    let setup = setup();
    let env = passing_env();
    let steps = steps();
    let oversized = "x".repeat(clio_types::CONTEXT_MAX_BYTES + 1);
    let mut big = task_args(&setup, &env, "task-big", "def", &steps, "s");
    big.context = Some(oversized.as_str());
    assert_eq!(task_upsert(&big).unwrap_err().code, ErrorCode::OutOfRange);
    let mut empty = task_args(&setup, &env, "task-empty", "def", &steps, "s");
    empty.context = Some("");
    assert_eq!(
        task_upsert(&empty).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert!(
        setup
            .store
            .get_task(setup.ctx.bank.as_str(), "task-big")
            .unwrap()
            .is_none()
    );
    assert!(
        setup.admission.events().is_empty(),
        "no admission decision for a pre-gate validation failure"
    );
}
