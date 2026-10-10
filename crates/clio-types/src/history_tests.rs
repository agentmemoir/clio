// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the history shapes and the
//! bounded-read `HistoryPolicy`.
//!
//! ## Owns
//! - `TaskRecord` / `FailureRecord` structural validation
//! - `HistoryPolicy` defaults, limit resolution, and lesson cap checks
//!
//! ## Does not own
//! - Persistence, encryption, or the tool-level history behavior
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::CONTEXT_MAX_BYTES;
use crate::error::ErrorCode;
use crate::history::{
    DEFAULT_HISTORY_LIMIT, DEFAULT_LESSON_MAX_TOKENS, DEFAULT_TASK_SIMILARITY_THRESHOLD,
    FailureRecord, HistoryPolicy, MAX_HISTORY_LIMIT, TaskRecord,
};
use crate::item::EpistemicKind;

fn task() -> TaskRecord {
    TaskRecord {
        id: "t1".into(),
        bank_id: "b".into(),
        item_id: "i1".into(),
        task_id: "task-a".into(),
        subject_id: "s".into(),
        version: 1,
        definition: "fix flaky login".into(),
        steps: vec!["run tests".into()],
        status: "in_progress".into(),
        evidence_ref: Some("turn-1".into()),
        context: None,
        admission_score: 0.7,
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn failure() -> FailureRecord {
    FailureRecord {
        id: "f1".into(),
        bank_id: "b".into(),
        item_id: "i2".into(),
        task_id: "task-a".into(),
        subject_id: "s".into(),
        attempt_n: 1,
        what_failed: "assertion error".into(),
        lesson: "re-read the fixture".into(),
        lesson_tokens: 4,
        evidence_ref: None,
        context: None,
        admission_score: 0.6,
        epistemic_kind: EpistemicKind::Belief,
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn record_serialization_omits_absent_context() {
    // Absent context serializes no key at all (no empty-string sentinel).
    let bare_task = serde_json::to_value(task()).unwrap();
    assert!(bare_task.get("context").is_none());
    let bare_failure = serde_json::to_value(failure()).unwrap();
    assert!(bare_failure.get("context").is_none());

    // A present context round-trips, and a payload without the key still
    // deserializes via the serde default.
    let mut t = task();
    t.context = Some("team chat".into());
    let mut payload = serde_json::to_value(&t).unwrap();
    assert_eq!(payload["context"], "team chat");
    payload
        .as_object_mut()
        .expect("task record is an object")
        .remove("context");
    let back: TaskRecord = serde_json::from_value(payload).unwrap();
    assert!(back.context.is_none());

    let mut f = failure();
    f.context = Some("attempt 2".into());
    let f_back: FailureRecord = serde_json::from_value(serde_json::to_value(&f).unwrap()).unwrap();
    assert_eq!(f_back.context.as_deref(), Some("attempt 2"));
}

#[test]
#[allow(clippy::float_cmp)]
fn policy_defaults_and_validation() {
    let p = HistoryPolicy::default();
    assert_eq!(p.lesson_max_tokens, DEFAULT_LESSON_MAX_TOKENS);
    assert_eq!(p.similarity_threshold, DEFAULT_TASK_SIMILARITY_THRESHOLD);
    assert_eq!(p.default_limit, DEFAULT_HISTORY_LIMIT);
    assert_eq!(p.max_limit, MAX_HISTORY_LIMIT);
    assert!(HistoryPolicy::new(0, 0.5, 1, 2).is_err());
    assert!(HistoryPolicy::new(10, 1.5, 1, 2).is_err());
    assert!(HistoryPolicy::new(10, 0.5, 0, 2).is_err());
    assert!(HistoryPolicy::new(10, 0.5, 3, 2).is_err());
    let ok = HistoryPolicy::new(10, 0.5, 1, 2).unwrap();
    assert_eq!(ok.max_limit, 2);
}

#[test]
fn policy_resolves_limits_and_caps_lessons() {
    let p = HistoryPolicy::new(4, 0.5, 5, 10).unwrap();
    assert_eq!(p.resolve_limit(None).unwrap(), 5);
    assert_eq!(p.resolve_limit(Some(3)).unwrap(), 3);
    assert_eq!(p.resolve_limit(Some(99)).unwrap(), 10);
    assert_eq!(
        p.resolve_limit(Some(0)).unwrap_err().code,
        ErrorCode::OutOfRange
    );
    assert_eq!(p.check_lesson("hello world").unwrap(), 3);
    let long = "x".repeat(400);
    let err = p.check_lesson(&long).unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
    assert!(err.message.contains("over the 4 token cap"));
}

#[test]
fn task_record_validates() {
    assert!(task().validate().is_ok());
    let mut t = task();
    t.id = String::new();
    assert_eq!(t.validate().unwrap_err().code, ErrorCode::InvalidArgument);
    let mut t = task();
    t.version = 0;
    assert_eq!(t.validate().unwrap_err().code, ErrorCode::OutOfRange);
    let mut t = task();
    t.definition = String::new();
    assert_eq!(t.validate().unwrap_err().code, ErrorCode::InvalidArgument);
    let mut t = task();
    t.status = String::new();
    assert_eq!(t.validate().unwrap_err().code, ErrorCode::InvalidArgument);
    let mut t = task();
    t.admission_score = 2.0;
    assert_eq!(t.validate().unwrap_err().code, ErrorCode::OutOfRange);
}

#[test]
fn failure_record_validates_and_requires_belief() {
    assert!(failure().validate().is_ok());
    let mut f = failure();
    f.attempt_n = 0;
    assert_eq!(f.validate().unwrap_err().code, ErrorCode::OutOfRange);
    let mut f = failure();
    f.what_failed = String::new();
    assert_eq!(f.validate().unwrap_err().code, ErrorCode::InvalidArgument);
    let mut f = failure();
    f.epistemic_kind = EpistemicKind::Fact;
    assert_eq!(f.validate().unwrap_err().code, ErrorCode::InvalidArgument);
    let mut f = failure();
    f.lesson.clear();
    // An empty lesson is still structurally valid; the cap policy owns the write.
    assert!(f.validate().is_ok());
    let mut f = failure();
    f.admission_score = f64::NAN;
    assert_eq!(f.validate().unwrap_err().code, ErrorCode::OutOfRange);
}

#[test]
fn task_record_context_validation() {
    // `context` obeys present values must be non-empty and at
    // most CONTEXT_MAX_BYTES UTF-8 bytes; `None` stays valid.
    let mut t = task();
    t.context = Some("team chat".into());
    t.validate().unwrap();
    t.context = Some(String::new());
    assert_eq!(t.validate().unwrap_err().code, ErrorCode::InvalidArgument);
    t.context = Some("x".repeat(CONTEXT_MAX_BYTES + 1));
    assert_eq!(t.validate().unwrap_err().code, ErrorCode::OutOfRange);
    t.context = Some("x".repeat(CONTEXT_MAX_BYTES));
    t.validate().unwrap();
    t.context = None;
    t.validate().unwrap();
}

#[test]
fn failure_record_context_validation() {
    let mut f = failure();
    f.context = Some("attempt 2".into());
    f.validate().unwrap();
    f.context = Some(String::new());
    assert_eq!(f.validate().unwrap_err().code, ErrorCode::InvalidArgument);
    f.context = Some("x".repeat(CONTEXT_MAX_BYTES + 1));
    assert_eq!(f.validate().unwrap_err().code, ErrorCode::OutOfRange);
    f.context = Some("x".repeat(CONTEXT_MAX_BYTES));
    f.validate().unwrap();
    f.context = None;
    f.validate().unwrap();
}
