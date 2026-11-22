// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the record-shaped batch
//! pre-flight helpers: the staged rows they build for persona, task, and
//! failure records, their refusal paths, and admission-decision rejection
//! mapping.
//!
//! ## Owns
//! - Staged-row assertions for `persona_put_stable`, `task_upsert`, and
//!   `failure_record` pre-flights
//! - The in-batch duplicate rejection each record pre-flight reports
//! - Refusal shapes: missing arguments and an unknown batch tool
//!
//! ## Does not own
//! - Batch op decoding or `$ref` resolution (see `batch_preflight.rs`)
//! - Transactional commit (see `clio-store`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::runtime::{BatchSignals, McpOpenOptions};
use clio_admission::decision::Decision;
use clio_store::ItemInspectFilter;
use clio_types::MemoryItem;
use serde_json::json;
use std::sync::Arc;

const BANK: &str = "bank-a";
const STAMP: &str = "2026-10-12T08:30:01Z";

/// One in-memory state; each test builds its own, so no test depends on another.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state")
}

/// Rows the bank holds, so a test can prove a pre-flight wrote nothing.
fn count(state: &McpState) -> usize {
    state
        .store
        .count_items(&ItemInspectFilter::new(BANK))
        .expect("count")
}

/// The signals a batch op scores against, carrying the items already staged.
fn signals<'a>(state: &McpState, staged: &'a [MemoryItem]) -> BatchSignals<'a> {
    BatchSignals::new(Arc::clone(&state.store), Arc::clone(&state.index), staged)
}

fn persona_params() -> Value {
    json!({
        "key": "communication_style", "value": "terse", "confidence": 0.9,
        "source": "explicit", "evidence_ref": "turn-1",
        "bank": BANK, "actor": "agent"
    })
}

fn task_params() -> Value {
    json!({
        "task_id": "task-1", "definition": "ship the fix", "status": "open",
        "steps": ["read the report", "write the test"],
        "bank": BANK, "actor": "agent"
    })
}

fn failure_params() -> Value {
    json!({
        "task_id": "task-1", "attempt_n": 2, "what_failed": "the gate refused",
        "lesson": "stage the record before scoring it",
        "bank": BANK, "actor": "agent"
    })
}

/// An item already staged earlier in the same batch, in the bank under test.
fn staged_task_item(s: &McpState) -> MemoryItem {
    let params = task_params();
    let ctx = s.resolve_ctx(&params).expect("ctx");
    let staged = records_preflight(s, "task_upsert", &ctx, STAMP, &params, &signals(s, &[]))
        .expect("a staged task record");
    match &staged.writes[0] {
        BatchWrite::CommitTaskUpsert { item, .. } => item.clone(),
        other => panic!("task_upsert must stage a commit, got {other:?}"),
    }
}

/// The refusal a pre-flight reported, or a panic naming the staged result.
fn refusal(result: Result<Staged, AmError>) -> AmError {
    match result {
        Ok(staged) => panic!(
            "expected a refusal, staged {} row effect(s)",
            staged.writes.len()
        ),
        Err(err) => err,
    }
}

#[test]
fn rejection_helper_maps_decision_reason() {
    let decision = Decision {
        pass: false,
        admission_score: 0.2,
        factors: clio_admission::decision::FactorBreakdown {
            utility: 0.1,
            confidence: 0.1,
            novelty: 0.1,
            type_prior: 0.1,
        },
        rejection_reason: Some("below theta".into()),
        profile_version: 1,
        mission_rule_ids: Vec::new(),
        mission_rule_labels: Vec::new(),
    };
    let err = rejection(&decision);
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    assert_eq!(err.message, "below theta");

    // A decision with no explicit reason falls back to the default message.
    let blank = rejection(&Decision {
        rejection_reason: None,
        ..decision
    });
    assert_eq!(blank.message, "admission rejected");
}

#[test]
fn persona_preflight_stages_the_entry_and_persists_nothing() {
    let s = state();
    let params = persona_params();
    let ctx = s.resolve_ctx(&params).expect("ctx");
    let staged = records_preflight(
        &s,
        "persona_put_stable",
        &ctx,
        STAMP,
        &params,
        &signals(&s, &[]),
    )
    .expect("a staged persona entry");
    assert_eq!(staged.writes.len(), 1);
    match &staged.writes[0] {
        BatchWrite::PutStableEntry { entry, actor } => {
            assert_eq!(entry.key, "communication_style");
            assert_eq!(entry.value, "terse");
            assert_eq!(actor, "agent");
        }
        other => panic!("persona_put_stable must stage a stable entry, got {other:?}"),
    }
    assert!(staged.id.is_none(), "a stable entry produces no item id");
    assert!(staged.admission_score.is_some(), "a scored op reports it");
    assert_eq!(count(&s), 0, "pre-flight scores, it does not write");
}

#[test]
fn a_task_record_repeated_in_the_same_batch_is_rejected() {
    let s = state();
    let params = task_params();
    let ctx = s.resolve_ctx(&params).expect("ctx");
    let earlier = [staged_task_item(&s)];
    let err = refusal(records_preflight(
        &s,
        "task_upsert",
        &ctx,
        STAMP,
        &params,
        &signals(&s, &earlier),
    ));
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        err.message.contains("exact_duplicate"),
        "the refusal must name the duplicate gate: {}",
        err.message
    );
    assert_eq!(count(&s), 0);
}

#[test]
fn task_preflight_stages_the_commit_with_the_steps_it_was_given() {
    let s = state();
    let params = task_params();
    let ctx = s.resolve_ctx(&params).expect("ctx");
    let staged = records_preflight(&s, "task_upsert", &ctx, STAMP, &params, &signals(&s, &[]))
        .expect("a staged task record");
    assert_eq!(staged.writes.len(), 1);
    match &staged.writes[0] {
        BatchWrite::CommitTaskUpsert { record, actor, .. } => {
            assert_eq!(record.task_id, "task-1");
            assert_eq!(record.definition, "ship the fix");
            assert_eq!(
                record.steps,
                ["read the report", "write the test"].map(str::to_owned)
            );
            assert_eq!(actor, "agent");
            assert_eq!(staged.id.as_deref(), Some(record.item_id.as_str()));
        }
        other => panic!("task_upsert must stage a commit, got {other:?}"),
    }
    assert!(staged.admission_score.is_some());
    assert_eq!(count(&s), 0);
}

#[test]
fn task_preflight_stages_an_empty_step_list_when_none_was_given() {
    let s = state();
    let mut params = task_params();
    params.as_object_mut().expect("params").remove("steps");
    let ctx = s.resolve_ctx(&params).expect("ctx");
    let staged = records_preflight(&s, "task_upsert", &ctx, STAMP, &params, &signals(&s, &[]))
        .expect("a staged task record");
    match &staged.writes[0] {
        BatchWrite::CommitTaskUpsert { record, .. } => {
            assert_eq!(record.steps.len(), 0, "{:?}", record.steps);
        }
        other => panic!("task_upsert must stage a commit, got {other:?}"),
    }
}

#[test]
fn failure_preflight_stages_the_commit_for_its_attempt() {
    let s = state();
    let params = failure_params();
    let ctx = s.resolve_ctx(&params).expect("ctx");
    let staged = records_preflight(
        &s,
        "failure_record",
        &ctx,
        STAMP,
        &params,
        &signals(&s, &[]),
    )
    .expect("a staged failure record");
    assert_eq!(staged.writes.len(), 1);
    match &staged.writes[0] {
        BatchWrite::CommitFailureRecord { record, actor, .. } => {
            assert_eq!(record.task_id, "task-1");
            assert_eq!(record.attempt_n, 2);
            assert_eq!(record.what_failed, "the gate refused");
            assert_eq!(actor, "agent");
            assert_eq!(staged.id.as_deref(), Some(record.item_id.as_str()));
        }
        other => panic!("failure_record must stage a commit, got {other:?}"),
    }
    assert!(staged.admission_score.is_some());
    assert_eq!(count(&s), 0);
}

#[test]
fn failure_preflight_refuses_a_missing_attempt_number() {
    let s = state();
    let mut params = failure_params();
    params.as_object_mut().expect("params").remove("attempt_n");
    let ctx = s.resolve_ctx(&params).expect("ctx");
    let err = refusal(records_preflight(
        &s,
        "failure_record",
        &ctx,
        STAMP,
        &params,
        &signals(&s, &[]),
    ));
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("attempt_n"), "{}", err.message);
    assert_eq!(count(&s), 0);
}

#[test]
fn task_preflight_refuses_a_missing_definition() {
    let s = state();
    let mut params = task_params();
    params.as_object_mut().expect("params").remove("definition");
    let ctx = s.resolve_ctx(&params).expect("ctx");
    let err = refusal(records_preflight(
        &s,
        "task_upsert",
        &ctx,
        STAMP,
        &params,
        &signals(&s, &[]),
    ));
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("definition"), "{}", err.message);
}

#[test]
fn records_preflight_refuses_an_unknown_batch_tool() {
    let s = state();
    let params = persona_params();
    let ctx = s.resolve_ctx(&params).expect("ctx");
    let err = refusal(records_preflight(
        &s,
        "no_such_tool",
        &ctx,
        STAMP,
        &params,
        &signals(&s, &[]),
    ));
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(err.message.contains("no_such_tool"), "{}", err.message);
}
