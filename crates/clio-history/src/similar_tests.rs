// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the lexical task-similarity tests and the
//! retitle fixture-set shape checks.
//!
//! ## Owns
//! - Token normalization, similarity scoring, ranking, and fixture shape
//!
//! ## Does not own
//! - Retry surfacing (`retry_tests`) or store reads
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::similar::{RETITLE_FIXTURES, lexical_similarity, normalize_tokens, rank_similar_tasks};
use clio_types::{HistoryPolicy, TaskRecord};

fn record(task_id: &str, definition: &str, created_at: &str) -> TaskRecord {
    TaskRecord {
        context: None,
        id: format!("id-{task_id}"),
        bank_id: "b".into(),
        item_id: "i".into(),
        task_id: task_id.into(),
        subject_id: "s".into(),
        version: 1,
        definition: definition.into(),
        steps: vec![],
        status: "done".into(),
        evidence_ref: None,
        admission_score: 0.5,
        created_at: created_at.into(),
    }
}

#[test]
fn tokens_and_similarity() {
    assert!(normalize_tokens("Fix Flaky-Login!").contains("flaky"));
    assert_eq!(lexical_similarity("", ""), 1.0);
    assert_eq!(lexical_similarity("alpha", ""), 0.0);
    assert_eq!(
        lexical_similarity("fix the flaky login test", "fix the flaky login test"),
        1.0
    );
    assert!(lexical_similarity("fix the flaky login test", "add retry to upload") < 0.1);
    let partial = lexical_similarity("fix the flaky login test", "fix the login test now");
    assert!(partial > 0.5 && partial < 1.0);
}

#[test]
fn rank_filters_and_orders() {
    let policy = HistoryPolicy::new(120, 0.5, 5, 10).unwrap();
    let tasks = vec![
        record("close", "fix the flaky login test", "2026-01-02"),
        record("exact", "fix the flaky login test", "2026-01-01"),
        record("far", "add retry to the upload client", "2026-01-03"),
    ];
    let ranked = rank_similar_tasks("fix the flaky login test", &tasks, &policy);
    assert_eq!(ranked.len(), 2);
    // Both exact-definition tasks score 1.0; the newer one ranks first.
    assert_eq!(ranked[0].1.task_id, "close");
    assert_eq!(ranked[1].1.task_id, "exact");

    let strict = HistoryPolicy::new(120, 1.0, 5, 10).unwrap();
    let exact_only = rank_similar_tasks("fix the flaky login test", &tasks, &strict);
    assert_eq!(exact_only.len(), 2);
}

#[test]
fn retitle_fixtures_are_distinct_ids_with_shared_definition() {
    assert!(RETITLE_FIXTURES.len() >= 5);
    for fixture in RETITLE_FIXTURES {
        assert_ne!(fixture.prior_task_id, fixture.retitled_task_id);
        assert!(!fixture.definition.is_empty(), "{:?}", fixture.definition);
    }
}
