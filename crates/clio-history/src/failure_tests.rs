// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `failure_record` write and
//! `failures_for_task` read tests, including caps and pre-gate validation
//!
//! ## Owns
//! - Admission pass/reject, lesson cap, duplicate, and bank/field validation
//! - Pre-gate `attempt_n` rejection and uncapped `what_failed` behavior
//!
//! ## Does not own
//! - Retry surfacing (`retry_tests`) or store-level history SQL
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::test_support::{failure_args, passing_env, query_args, setup};
use crate::{FailureRecordOutcome, failure_record, failures_for_task};
use clio_store::{HistoryStore, Store};
use clio_types::{ErrorCode, HistoryPolicy};

#[test]
fn t03_failure_record_stored_and_readable() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let out = failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "task-a",
        2,
        "assertion error in login spec",
        "re-read the fixture before patching",
    ))
    .unwrap();
    let FailureRecordOutcome::Stored { record, decision } = out else {
        panic!("expected stored");
    };
    assert!(decision.pass);
    assert_eq!(record.attempt_n, 2);
    assert_eq!(record.lesson, "re-read the fixture before patching");
    assert!(record.lesson_tokens > 0);
    assert_eq!(
        record.epistemic_kind,
        clio_types::EpistemicKind::Belief,
        "a failure is a belief"
    );

    let read = setup
        .store
        .get_failure(setup.ctx.bank.as_str(), &record.id)
        .unwrap()
        .expect("readable");
    assert_eq!(read.what_failed, "assertion error in login spec");
    assert_eq!(read.admission_score, record.admission_score);

    let cipher = setup
        .store
        .inspect_content_ciphertext(setup.ctx.bank.as_str(), &record.item_id)
        .unwrap()
        .expect("ciphertext");
    assert!(!cipher.contains("fixture"));
    assert_eq!(setup.sink.events()[0].outcome, "stored");
    assert_eq!(setup.sink.events()[0].operation, "failure_record");
}

#[test]
fn t04_lesson_over_cap_is_rejected_and_stored_nowhere() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::new(4, 0.5, 5, 10).unwrap();
    let long = "x".repeat(400);
    let err = failure_record(&failure_args(
        &setup, &env, &policy, "task-a", 1, "boom", &long,
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
    let stored = setup
        .store
        .failures_for_task_exact(setup.ctx.bank.as_str(), "task-a", 10)
        .unwrap();
    assert!(stored.is_empty(), "over-cap lesson must not be stored");
}

#[test]
fn t05_gate_reject_leaves_no_record() {
    let setup = setup();
    let env = crate::test_support::failing_env();
    let policy = HistoryPolicy::default();
    let out = failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "task-a",
        1,
        "boom",
        "short lesson",
    ))
    .unwrap();
    assert!(matches!(out, FailureRecordOutcome::Rejected { .. }));
    let stored = setup
        .store
        .failures_for_task_exact(setup.ctx.bank.as_str(), "task-a", 10)
        .unwrap();
    assert!(stored.is_empty(), "{stored:?}");
    assert!(setup.sink.events()[0].rejection_reason.is_some());
}

#[test]
fn t06_failures_for_task_is_newest_first() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    for attempt in 1..=2 {
        failure_record(&failure_args(
            &setup,
            &env,
            &policy,
            "task-a",
            attempt,
            "boom",
            "short lesson",
        ))
        .unwrap();
    }
    let out = failures_for_task(&query_args(&setup, &policy, "task-a", Some(10))).unwrap();
    assert_eq!(out.mode, "exact");
    assert_eq!(out.failures.len(), 2);
    assert_eq!(out.failures[0].attempt_n, 2);
    assert_eq!(out.failures[1].attempt_n, 1);
    assert_eq!(setup.sink.events().last().unwrap().record_count, 2);
}

#[test]
fn duplicate_attempt_is_rejected() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "task-a",
        1,
        "boom",
        "lesson one",
    ))
    .unwrap();
    let err = failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "task-a",
        1,
        "boom again",
        "lesson two",
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn failure_validates_required_fields() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let err = failure_record(&failure_args(&setup, &env, &policy, "", 1, "x", "y")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = failure_record(&failure_args(&setup, &env, &policy, "t", 1, "", "y")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = failure_record(&failure_args(&setup, &env, &policy, "t", 0, "x", "y")).unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
}

#[test]
fn attempt_n_is_validated_before_the_gate() {
    // A bad attempt_n must fail before the gate, so no admit event is
    // emitted for a record that is never written.
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let err = failure_record(&failure_args(&setup, &env, &policy, "t", 0, "x", "y")).unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
    assert!(
        setup.admission.events().is_empty(),
        "no admission decision for a pre-gate validation failure"
    );
    let gated = setup
        .store
        .failures_for_task_exact(setup.ctx.bank.as_str(), "t", 10)
        .unwrap();
    assert!(gated.is_empty(), "{gated:?}");
}

#[test]
fn what_failed_is_uncapped_while_lesson_is_capped() {
    // Only `lesson` is length-capped; `what_failed` is the faithful
    // factual record and round-trips unchanged.
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::new(4, 0.5, 5, 10).unwrap();
    let big = "e".repeat(2_000);
    let out = failure_record(&failure_args(
        &setup, &env, &policy, "task-a", 1, &big, "ok",
    ))
    .expect("uncapped what_failed stores");
    let FailureRecordOutcome::Stored { record, .. } = out else {
        panic!("expected stored");
    };
    assert_eq!(record.what_failed.len(), 2_000);
    let read = setup
        .store
        .get_failure(setup.ctx.bank.as_str(), &record.id)
        .unwrap()
        .expect("readable");
    assert_eq!(read.what_failed.len(), 2_000);
    // The lesson cap still applies: over-cap lesson is rejected outright.
    let err = failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "task-b",
        1,
        "boom",
        &"x".repeat(400),
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
}

#[test]
fn dry_run_scores_and_stages_without_persisting() {
    let s = crate::test_support::setup();
    let env = crate::test_support::passing_env();
    let policy = HistoryPolicy::default();
    let mut args =
        crate::test_support::failure_args(&s, &env, &policy, "task-dry", 1, "boom", "short lesson");
    args.dry_run = true;
    match failure_record(&args).unwrap() {
        FailureRecordOutcome::DryRun {
            item,
            record,
            decision,
        } => {
            assert!(decision.pass);
            assert_eq!(item.id, record.item_id);
        }
        _ => panic!("expected dry-run outcome"),
    }
    let dry_run_stored = s.store.list_failures(s.ctx.bank.as_str(), 10).unwrap();
    assert!(dry_run_stored.is_empty(), "{dry_run_stored:?}");
}

/// A supplied context is sealed into the carrier item and surfaced by the
/// domain reads; a record without context serializes no `context` key.
#[test]
fn context_is_sealed_into_the_carrier_and_surfaced() {
    let setup = setup();
    let env = passing_env();
    let policy = HistoryPolicy::default();
    let mut args = failure_args(&setup, &env, &policy, "task-a", 2, "boom", "short lesson");
    args.context = Some("attempt 2 of 3");
    let FailureRecordOutcome::Stored { record, .. } = failure_record(&args).unwrap() else {
        panic!("expected stored");
    };
    assert_eq!(record.context.as_deref(), Some("attempt 2 of 3"));
    let read = setup
        .store
        .get_failure(setup.ctx.bank.as_str(), &record.id)
        .unwrap()
        .expect("readable");
    assert_eq!(read.context.as_deref(), Some("attempt 2 of 3"));

    let out = failure_record(&failure_args(
        &setup,
        &env,
        &policy,
        "task-b",
        1,
        "boom",
        "short lesson",
    ))
    .unwrap();
    let FailureRecordOutcome::Stored { record, .. } = out else {
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
    let policy = HistoryPolicy::default();
    let oversized = "x".repeat(clio_types::CONTEXT_MAX_BYTES + 1);
    let mut big = failure_args(&setup, &env, &policy, "task-big", 1, "boom", "lesson");
    big.context = Some(oversized.as_str());
    assert_eq!(
        failure_record(&big).unwrap_err().code,
        ErrorCode::OutOfRange
    );
    let mut empty = failure_args(&setup, &env, &policy, "task-empty", 1, "boom", "lesson");
    empty.context = Some("");
    assert_eq!(
        failure_record(&empty).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let stored = setup
        .store
        .failures_for_task_exact(setup.ctx.bank.as_str(), "task-big", 10)
        .unwrap();
    assert!(stored.is_empty(), "{stored:?}");
    assert!(
        setup.admission.events().is_empty(),
        "no admission decision for a pre-gate validation failure"
    );
}
