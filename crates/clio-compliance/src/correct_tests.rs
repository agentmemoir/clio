// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `correct` routing tests: reason validation,
//! discrete supersession, continuous re-centering, and fail-closed rejections.
//!
//! ## Owns
//! - Test doubles and assertions for the correction service
//!
//! ## Does not own
//! - Production routing (`correct`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::correct::{CorrectArgs, correct};
use clio_store::{BeliefStore, ContinuousStore, Store};
use clio_types::{ErrorCode, ItemKind, MemoryItem, SemanticCategory, UpdateRule};
use serde_json::json;

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.7),
        admission_score: Some(0.8),
        source_ref: Some("turn-1".into()),
        snapshot: Some(json!({"meeting": "2pm"})),
        gist: Some("meeting at 2pm".into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    }
}

fn fixture() -> (clio_store::SqliteStore, String, String) {
    let store = clio_store::SqliteStore::open_in_memory(std::sync::Arc::new(
        clio_store::LocalDevKms::new(),
    ))
    .expect("sqlite store");
    let bank = format!("bank-correct-{}", std::process::id());
    let id = "item-a";
    store
        .create_memory_item(&item(id, &bank), "agent")
        .expect("seed item");
    (store, bank, id.to_owned())
}

fn args<'a>(
    store: &'a clio_store::SqliteStore,
    bank: &'a str,
    target: &'a str,
    new_value: &'a str,
    reason: &'a str,
    new_id: &'a str,
    ts: &'a str,
) -> CorrectArgs<'a> {
    CorrectArgs {
        store,
        continuous: store,
        bank_id: bank,
        actor: "agent",
        item_id: target,
        new_value,
        reason,
        context: None,
        timestamp: ts,
        new_item_id: new_id,
    }
}

#[test]
fn t18_05_blank_reason_fails_closed() {
    let (store, bank, id) = fixture();
    for reason in ["", "   "] {
        let err = correct(&args(
            &store,
            &bank,
            &id,
            "meeting at 3pm",
            reason,
            "item-b",
            "t2",
        ))
        .expect_err("blank reason rejected");
        assert_eq!(err.code, ErrorCode::InvalidArgument);
    }
    // No state change: the prior is still open.
    assert!(
        store
            .get_memory_item(&bank, &id)
            .expect("get")
            .expect("exists")
            .tx_until
            .is_none()
    );
}

#[test]
fn missing_item_is_item_not_found() {
    let (store, bank, _id) = fixture();
    let err = correct(&args(&store, &bank, "nope", "v", "fix", "item-z", "t2"))
        .expect_err("missing item rejected");
    assert_eq!(err.code, ErrorCode::ItemNotFound);
}

#[test]
fn empty_scope_args_fail_closed() {
    let (store, bank, id) = fixture();
    assert_eq!(
        correct(&args(&store, "", &id, "v", "r", "item-b", "t2"))
            .expect_err("empty bank")
            .code,
        ErrorCode::MissingBank
    );
    assert_eq!(
        correct(&args(&store, &bank, "", "v", "r", "item-b", "t2"))
            .expect_err("empty target")
            .code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn declared_discrete_attribute_key_is_rejected() {
    let (store, bank, _id) = fixture();
    store
        .put_attribute_schema(
            &clio_types::AttributeSchema {
                bank_id: bank.clone(),
                attr_key: "membership".into(),
                update_rule: UpdateRule::Discrete,
                alpha: Some(0.5),
                trend_window: None,
                bounds_lo: None,
                bounds_hi: None,
                clamp_values: false,
                created_at: "2024-01-01T00:00:00Z".into(),
                updated_at: None,
            },
            "agent",
        )
        .expect("schema");
    let err = correct(&args(&store, &bank, "membership", "gold", "r", "x", "t2"))
        .expect_err("discrete attribute key rejected");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn string_new_value_keeps_structured_snapshot() {
    let (store, bank, id) = fixture();
    correct(&args(
        &store,
        &bank,
        &id,
        "meeting moved to 3pm",
        "prose fix",
        "item-b",
        "t2",
    ))
    .expect("prose correction");
    let active = store
        .get_memory_item(&bank, "item-b")
        .expect("new readable")
        .expect("new exists");
    assert_eq!(active.gist.as_deref(), Some("meeting moved to 3pm"));
    assert_eq!(
        active.snapshot.as_ref().expect("snapshot retained")["meeting"],
        "2pm"
    );
}

#[test]
fn t18_03_discrete_correction_supersedes_on_both_axes() {
    let (store, bank, id) = fixture();
    let out = correct(&args(
        &store,
        &bank,
        &id,
        r#"{"meeting": "3pm"}"#,
        "user corrected the time",
        "item-b",
        "2024-06-01T00:00:00Z",
    ))
    .expect("discrete correction");
    let correction = out.correction.expect("discrete outcome");
    assert_eq!(correction.new_item_id, "item-b");
    assert!(!correction.already_superseded);
    // Prior closed on both axes, still queryable as past system belief.
    let prior = store
        .get_memory_item(&bank, &id)
        .expect("prior readable")
        .expect("prior exists");
    assert_eq!(prior.tx_until.as_deref(), Some("2024-06-01T00:00:00Z"));
    assert_eq!(
        prior.snapshot.as_ref().expect("prior snapshot")["meeting"],
        "2pm"
    );
    // New assertion active with the corrected value and carried provenance.
    let active = store
        .get_memory_item(&bank, "item-b")
        .expect("new readable")
        .expect("new exists");
    assert!(active.tx_until.is_none());
    assert_eq!(active.gist.as_deref(), Some(r#"{"meeting": "3pm"}"#));
    assert_eq!(
        active.snapshot.as_ref().expect("new snapshot")["meeting"],
        "3pm"
    );
    assert_eq!(active.admission_score, Some(0.8));
    // Correcting the already-superseded prior fails closed.
    let err = correct(&args(&store, &bank, &id, "x", "again", "item-c", "t3"))
        .expect_err("superseded prior rejected");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn t18_01_t18_04_telemetry_and_past_queryability() {
    let (store, bank, id) = fixture();
    correct(&args(
        &store,
        &bank,
        &id,
        r#"{"meeting": "3pm"}"#,
        "user corrected the time",
        "item-b",
        "2024-06-01T00:00:00Z",
    ))
    .expect("correction");
    let trail = store.audit_trail(&bank, &id).expect("trail");
    assert_eq!(trail.lineage_ids, vec![id.clone(), "item-b".to_owned()]);
    let ops: Vec<&str> = trail.events.iter().map(|e| e.operation.as_str()).collect();
    assert!(ops.contains(&"create_memory_item"));
    assert!(ops.contains(&"correct"));
    let close_event = trail
        .events
        .iter()
        .find(|e| e.item_id == id && e.operation == "correct")
        .expect("prior close event");
    assert_eq!(close_event.actor, "agent");
    assert_eq!(close_event.detail["reason"], "user corrected the time");
    // New revision linked both ways.
    let rev_new = trail
        .revisions
        .iter()
        .find(|r| r.item_id == "item-b")
        .expect("rev");
    assert_eq!(rev_new.corrects_item_id.as_deref(), Some(id.as_str()));
    let rev_prior = trail
        .revisions
        .iter()
        .find(|r| r.item_id == id)
        .expect("prior rev");
    assert_eq!(rev_prior.superseded_by_id.as_deref(), Some("item-b"));
    assert_eq!(rev_prior.tx_until.as_deref(), Some("2024-06-01T00:00:00Z"));
}

#[test]
fn continuous_correction_recenters_and_records_reason() {
    let (store, bank, _id) = fixture();
    store
        .put_attribute_schema(
            &clio_types::AttributeSchema {
                bank_id: bank.clone(),
                attr_key: "affinity".into(),
                update_rule: UpdateRule::Continuous,
                alpha: Some(0.5),
                trend_window: None,
                bounds_lo: None,
                bounds_hi: None,
                clamp_values: false,
                created_at: "2024-01-01T00:00:00Z".into(),
                updated_at: None,
            },
            "agent",
        )
        .expect("schema");
    let out = correct(&args(
        &store,
        &bank,
        "affinity",
        "0.4",
        "operator adjusted",
        "x",
        "2024-01-02T00:00:00Z",
    ))
    .expect("continuous correction");
    let state = out.state.expect("continuous state");
    assert!((state.state - 0.4).abs() < 1e-9);
    // The correction reason is persisted on the same-transaction telemetry row
    // with operation `correct` (attribute keys are not item anchors, so
    // item-level audit_trail does not apply); clio-store parity tests assert the
    // persisted row directly.
    assert_eq!(out.rule, "continuous");
}

#[test]
fn continuous_non_numeric_fails_closed() {
    let (store, bank, _id) = fixture();
    store
        .put_attribute_schema(
            &clio_types::AttributeSchema {
                bank_id: bank.clone(),
                attr_key: "affinity".into(),
                update_rule: UpdateRule::Continuous,
                alpha: Some(0.5),
                trend_window: None,
                bounds_lo: None,
                bounds_hi: None,
                clamp_values: false,
                created_at: "2024-01-01T00:00:00Z".into(),
                updated_at: None,
            },
            "agent",
        )
        .expect("schema");
    let err = correct(&args(
        &store,
        &bank,
        "affinity",
        "not-a-number",
        "r",
        "x",
        "t2",
    ))
    .expect_err("non-numeric rejected");
    assert_eq!(err.code, ErrorCode::InvalidType);
    let err =
        correct(&args(&store, &bank, "affinity", "NaN", "r", "x", "t2")).expect_err("NaN rejected");
    assert_eq!(err.code, ErrorCode::OutOfRange);
}

#[test]
fn belief_targets_reject_correct() {
    let (store, bank, _id) = fixture();
    let mut belief = item("belief-x", &bank);
    belief.epistemic_kind = clio_types::EpistemicKind::Belief;
    belief.source_type = Some(clio_types::SourceType::UserStated);
    belief.category = None;
    belief.kind = ItemKind::Episodic;
    belief.episodic_type = Some(clio_types::EpisodicType::Gist);
    store
        .create_memory_item(&belief, "agent")
        .expect("belief item");
    let err = correct(&args(&store, &bank, "belief-x", "v", "r", "item-c", "t2"))
        .expect_err("belief target rejected");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

/// Real beliefs are not items at all: a value correction on a belief id
/// never routes (the authorized context change lives in
/// `correct_belief_context`), so the id is simply not an item target.
#[test]
fn real_belief_ids_are_not_item_correction_targets() {
    let (store, _kms, bank, id) = crate::correct::correct_context_tests::belief_fixture("kept");
    let err = correct(&args(&store, &bank, &id, "v", "r", "item-c", "t2"))
        .expect_err("real belief ids never route through correct");
    assert_eq!(err.code, ErrorCode::ItemNotFound);
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("kept"));
    assert_eq!(got.confidence_history.len(), 1);
}
