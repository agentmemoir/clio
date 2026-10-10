// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for source-`context` coverage of the `correct`
//! service: preservation when no replacement is supplied, audited replacement
//! with prior revisions still reconstructable, and fail-closed validation.
//!
//! ## Owns
//! - Context-focused correction assertions over an in-memory SQLite store
//!
//! ## Does not own
//! - Correction routing itself (see `correct.rs`)
//! - Audit-view masking (see `audit_context_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::audit::audit_trail;
use crate::correct::{BeliefContextCorrectArgs, CorrectArgs, correct, correct_belief_context};
use clio_store::{BeliefStore, ContinuousStore, DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{
    AttributeSchema, BeliefObject, ConfidenceEntry, ErrorCode, ItemKind, MemoryItem,
    SemanticCategory, SourceType, UpdateRule,
};
use serde_json::json;

fn item(id: &str, bank: &str, context: Option<&str>) -> MemoryItem {
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
        context: context.map(str::to_owned),
    }
}

fn fixture(context: &str) -> (SqliteStore, String, String) {
    let store = SqliteStore::open_in_memory(std::sync::Arc::new(clio_store::LocalDevKms::new()))
        .expect("sqlite store");
    let bank = format!("bank-correct-ctx-{}", std::process::id());
    let id = "item-a";
    store
        .create_memory_item(&item(id, &bank, Some(context)), "agent")
        .expect("seed item");
    (store, bank, id.to_owned())
}

fn args<'a>(
    store: &'a SqliteStore,
    bank: &'a str,
    target: &'a str,
    context: Option<&'a str>,
    new_item_id: &'a str,
    timestamp: &'a str,
) -> CorrectArgs<'a> {
    CorrectArgs {
        store,
        continuous: store,
        bank_id: bank,
        actor: "agent",
        item_id: target,
        new_value: "meeting at 3pm",
        reason: "operator corrected the context",
        context,
        timestamp,
        new_item_id,
    }
}

/// A real belief (never an item row) with a stored sealed context, plus
/// the KMS that sealed it, for the authorized belief-context correction path.
pub(crate) fn belief_fixture(
    context: &str,
) -> (SqliteStore, std::sync::Arc<dyn DekProvider>, String, String) {
    let kms: std::sync::Arc<dyn DekProvider> = std::sync::Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(std::sync::Arc::clone(&kms)).expect("sqlite store");
    let bank = format!("bank-belief-correct-{}", std::process::id());
    let id = "belief-x";
    let belief = BeliefObject {
        id: id.into(),
        bank_id: bank.clone(),
        subject_id: "subj-belief".into(),
        proposition: "User Is Likely Vegetarian".into(),
        context: Some(context.to_owned()),
        context_as_of: None,
        confidence_history: vec![
            ConfidenceEntry::new("2026-06-01", 0.4, "turn_112", SourceType::UserStated)
                .expect("entry"),
        ],
        created_at: "2026-06-01".into(),
        updated_at: "2026-06-01".into(),
    };
    store.create_belief(&belief, "agent").expect("seed belief");
    (store, kms, bank, id.to_owned())
}

#[test]
fn missing_context_preserves_the_prior_value() {
    let (store, bank, id) = fixture("team chat");
    correct(&args(
        &store,
        &bank,
        &id,
        None,
        "item-b",
        "2026-06-01T00:00:00Z",
    ))
    .expect("preserving correction");
    let active = store
        .get_memory_item(&bank, "item-b")
        .expect("new readable")
        .expect("new exists");
    assert_eq!(active.context.as_deref(), Some("team chat"));
    let trail = audit_trail(&store, &bank, &id).expect("trail");
    let prior = trail
        .revisions
        .iter()
        .find(|r| r["item_id"] == id.as_str())
        .expect("prior revision");
    assert_eq!(prior["context"], json!("team chat"));
}

#[test]
fn explicit_context_replaces_and_prior_revisions_stay_reconstructable() {
    let (store, bank, id) = fixture("original context");
    correct(&args(
        &store,
        &bank,
        &id,
        None,
        "item-b",
        "2026-06-01T00:00:00Z",
    ))
    .expect("preserving correction");
    correct(&args(
        &store,
        &bank,
        "item-b",
        Some("replacement context"),
        "item-c",
        "2026-07-01T00:00:00Z",
    ))
    .expect("replacement correction");
    let active = store
        .get_memory_item(&bank, "item-c")
        .expect("new readable")
        .expect("new exists");
    assert_eq!(active.context.as_deref(), Some("replacement context"));

    let trail = audit_trail(&store, &bank, &id).expect("trail");
    assert_eq!(trail.revisions.len(), 3);
    let context_of = |revision_id: &str| {
        trail
            .revisions
            .iter()
            .find(|r| r["item_id"] == revision_id)
            .map(|r| r["context"].clone())
    };
    assert_eq!(context_of(&id), Some(json!("original context")));
    assert_eq!(context_of("item-b"), Some(json!("original context")));
    assert_eq!(context_of("item-c"), Some(json!("replacement context")));
}

#[test]
fn context_on_the_continuous_route_is_rejected_without_a_write() {
    let (store, bank, _id) = fixture("team chat");
    store
        .put_attribute_schema(
            &AttributeSchema {
                bank_id: bank.clone(),
                attr_key: "pref.formality".into(),
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
    let err = correct(&CorrectArgs {
        store: &store,
        continuous: &store,
        bank_id: &bank,
        actor: "agent",
        item_id: "pref.formality",
        new_value: "0.7",
        reason: "recenter formality",
        context: Some("team chat"),
        timestamp: "2026-06-01T00:00:00Z",
        new_item_id: "item-b",
    })
    .expect_err("context on the EMA route must fail closed");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        !err.message.contains("team chat"),
        "the supplied context must not be echoed: {}",
        err.message
    );
    // Fail-closed: a declared EMA attribute keeps no item context, so the
    // correction is refused rather than silently dropping the value, and no
    // observation is written.
    let series = store
        .continuous_series(&bank, "pref.formality", None, 10)
        .expect("series");
    assert!(
        series.is_empty(),
        "no observation may be written: {series:?}"
    );

    // Omitting context still re-centers the EMA state.
    let out = correct(&CorrectArgs {
        store: &store,
        continuous: &store,
        bank_id: &bank,
        actor: "agent",
        item_id: "pref.formality",
        new_value: "0.7",
        reason: "recenter formality",
        context: None,
        timestamp: "2026-06-01T00:00:00Z",
        new_item_id: "item-b",
    })
    .expect("continuous correction without context");
    assert_eq!(out.rule, "continuous");
    assert_eq!(
        store
            .continuous_series(&bank, "pref.formality", None, 10)
            .expect("series")
            .len(),
        1
    );
}

#[test]
fn empty_or_oversized_replacement_is_rejected() {
    let (store, bank, id) = fixture("team chat");
    let err = correct(&args(
        &store,
        &bank,
        &id,
        Some(""),
        "item-b",
        "2026-06-01T00:00:00Z",
    ))
    .expect_err("empty context rejected");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let big = "a".repeat(4097);
    let err = correct(&args(
        &store,
        &bank,
        &id,
        Some(&big),
        "item-b",
        "2026-06-01T00:00:00Z",
    ))
    .expect_err("oversized context rejected");
    assert_eq!(err.code, ErrorCode::OutOfRange);
    // Fail-closed: the prior revision stays open and no corrected row exists.
    let prior = store
        .get_memory_item(&bank, &id)
        .expect("prior readable")
        .expect("prior exists");
    assert!(prior.tx_until.is_none());
    assert!(
        store
            .get_memory_item(&bank, "item-b")
            .expect("new readable")
            .is_none()
    );
}

/// The authorized belief-context correction: only the sealed context
/// changes, the trajectory is untouched, and the trail exposes hash-only
/// prior/new stats (presence, length, hash) — raw plaintext is absent and the
/// prior sealed value is not retained in the append-only audit at all
/// So it is not reconstructable from `audit_trail`.
#[test]
fn belief_context_correction_changes_only_context_and_records_no_prior_cipher() {
    let (store, _kms, bank, id) = belief_fixture("original context");
    let out = correct_belief_context(&BeliefContextCorrectArgs {
        beliefs: &store,
        bank_id: &bank,
        belief_id: &id,
        new_context: "replacement context",
        reason: "operator corrected the context",
        actor: "agent",
    })
    .expect("belief-context correction");
    assert_eq!(out.rule, "belief_context");
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("replacement context"));
    assert_eq!(got.confidence_history.len(), 1, "values evolve via appends");

    let trail = audit_trail(&store, &bank, &id).expect("trail");
    let event = trail
        .events
        .iter()
        .find(|e| e["operation"] == "correct")
        .expect("correction event on the belief trail");
    assert_eq!(event["item_id"], json!(id));
    assert_eq!(event["detail"]["corrected"], json!("context"));
    assert_eq!(event["detail"]["reason"], "operator corrected the context");
    assert_eq!(
        event["detail"]["prior_context_len"],
        "original context".len()
    );
    assert_eq!(event["detail"]["context_len"], "replacement context".len());
    let serialized = serde_json::to_string(&trail).unwrap();
    assert!(!serialized.contains("original context"), "{serialized}");
    assert!(!serialized.contains("replacement context"), "{serialized}");
    // The audit detail keeps presence/length/hash statistics only —
    // the sealed prior context envelope is never retained in the
    // append-only audit (the same rule the persona path follows).
    assert!(
        event["detail"].get("prior_context_cipher").is_none(),
        "{serialized}"
    );
    assert!(
        event["detail"]["prior_context_hash"].is_string(),
        "hash-only prior stats remain: {serialized}"
    );
}

/// Fail-closed inputs for the authorized belief-context correction: blank
/// reason, empty bank/target, and invalid context reject before any write.
fn belief_arg<'a>(
    store: &'a SqliteStore,
    bank: &'a str,
    belief: &'a str,
    context: &'a str,
    reason: &'a str,
) -> BeliefContextCorrectArgs<'a> {
    BeliefContextCorrectArgs {
        beliefs: store,
        bank_id: bank,
        belief_id: belief,
        new_context: context,
        reason,
        actor: "agent",
    }
}

#[test]
fn belief_context_correction_fails_closed_before_any_write() {
    let (store, _kms, bank, id) = belief_fixture("kept");
    assert_eq!(
        correct_belief_context(&belief_arg(&store, "", &id, "v", "r"))
            .unwrap_err()
            .code,
        ErrorCode::MissingBank
    );
    assert_eq!(
        correct_belief_context(&belief_arg(&store, &bank, " ", "v", "r"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        correct_belief_context(&belief_arg(&store, &bank, &id, "v", "   "))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        correct_belief_context(&belief_arg(&store, &bank, &id, "", "r"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    let got = store.get_belief(&bank, &id).unwrap().expect("belief");
    assert_eq!(got.context.as_deref(), Some("kept"));
    assert_eq!(got.confidence_history.len(), 1);
}
