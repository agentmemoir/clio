// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the collaborative reviewer
//! tool: attest/update/invalidate/discard actions with attributable audit rows
//! The action-semantics contract those tests exist to pin.
//!
//! ## Owns
//! - Action-semantics and note-requirement assertions for `validate`
//!
//! ## Does not own
//! - The credential label a durable row carries (`validate_principal_tests.rs`)
//! - The validator implementation itself (see `validate.rs`)
//! - MCP tool binding for `validate` (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::validate_test_support::{args, item, store, trail};
use crate::test_unique::unique;
use crate::validate::{ValidateAction, validate};
use clio_store::Store;
use clio_types::{Actor, ErrorCode};

#[test]
fn attest_records_telemetry_without_content_change() {
    let store = store();
    let bank = unique("b");
    let subject = unique("s");
    let item_id = unique("itm");
    store
        .create_memory_item(
            &clio_types::MemoryItem {
                bank_id: bank.clone(),
                ..item(&item_id, &bank, &subject)
            },
            "agent",
        )
        .unwrap();
    let out = validate(&args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Attest,
        Some("verified against the API docs"),
        None,
        &unique("itm"),
    ))
    .unwrap();
    assert_eq!(out.action, "attest");
    assert!(!out.applied);
    assert_eq!(out.actor, "harness");
    let after = store.get_memory_item(&bank, &item_id).unwrap().unwrap();
    assert_eq!(
        after.gist.as_deref(),
        Some("the API endpoint is https://api")
    );
    assert!(trail(&store, &bank, &item_id).contains(&"validate_attest".to_owned()));
    // Attestation without a note is legal.
    validate(&args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Attest,
        None,
        None,
        &unique("x"),
    ))
    .unwrap();
}

#[test]
fn invalidate_closes_axes_and_logs_reviewer_note() {
    let store = store();
    let bank = unique("b");
    let item_id = unique("itm");
    store
        .create_memory_item(&item(&item_id, &bank, "s"), "agent")
        .unwrap();
    let note = "stale after the endpoint moved";
    let out = validate(&args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Invalidate,
        Some(note),
        None,
        &unique("x"),
    ))
    .unwrap();
    assert_eq!(out.action, "invalidate");
    assert!(out.applied);
    let after = store.get_memory_item(&bank, &item_id).unwrap().unwrap();
    assert!(after.tx_until.is_some());
    assert!(after.valid_until.is_some());
    // Reviewer note is visible in the durable audit row.
    let trail = store.audit_trail(&bank, &item_id).unwrap();
    assert!(
        trail
            .events
            .iter()
            .any(|e| e.operation == "validate_invalidate" && e.detail.to_string().contains(note))
    );
}

#[test]
fn destructive_actions_require_note() {
    let store = store();
    let bank = unique("b");
    let item_id = unique("itm");
    store
        .create_memory_item(&item(&item_id, &bank, "s"), "agent")
        .unwrap();
    for action in [
        ValidateAction::Invalidate,
        ValidateAction::Discard,
        ValidateAction::Update,
    ] {
        let err = validate(&args(
            &store,
            &bank,
            &item_id,
            action,
            None,
            None,
            &unique("x"),
        ))
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument);
    }
    let err = validate(&args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Update,
        Some("fix"),
        None,
        &unique("x"),
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    // Nothing changed.
    assert!(
        store
            .get_memory_item(&bank, &item_id)
            .unwrap()
            .unwrap()
            .tx_until
            .is_none()
    );
}

#[test]
fn discard_marks_item_and_discard_action_is_logged() {
    let store = store();
    let bank = unique("b");
    let item_id = unique("itm");
    store
        .create_memory_item(&item(&item_id, &bank, "s"), "agent")
        .unwrap();
    let out = validate(&args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Discard,
        Some("operational noise"),
        None,
        &unique("x"),
    ))
    .unwrap();
    assert_eq!(out.action, "discard");
    assert!(out.applied);
    let ops = trail(&store, &bank, &item_id);
    assert!(ops.contains(&"discard".to_owned()));
    assert!(ops.contains(&"validate_discard".to_owned()));
}

#[test]
fn update_routes_through_correct() {
    let store = store();
    let bank = unique("b");
    let item_id = unique("itm");
    store
        .create_memory_item(&item(&item_id, &bank, "s"), "agent")
        .unwrap();
    let out = validate(&args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Update,
        Some("endpoint moved"),
        Some("the API endpoint is https://api2"),
        &unique("fix"),
    ))
    .unwrap();
    assert_eq!(out.action, "update");
    assert!(out.applied);
    // Discrete correction: prior closed, corrected revision linked.
    assert!(
        store
            .get_memory_item(&bank, &item_id)
            .unwrap()
            .unwrap()
            .tx_until
            .is_some()
    );
    assert!(trail(&store, &bank, &item_id).contains(&"correct".to_owned()));
}

#[test]
fn unknown_item_and_bad_action_fail_closed() {
    let store = store();
    let bank = unique("b");
    let err = validate(&args(
        &store,
        &bank,
        "missing-item",
        ValidateAction::Attest,
        None,
        None,
        &unique("x"),
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::ItemNotFound);
    let err = ValidateAction::parse("destroy").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        ValidateAction::parse("attest").unwrap(),
        ValidateAction::Attest
    );
}

#[test]
fn reviewer_note_secrets_are_masked_on_output() {
    let store = store();
    let bank = unique("b");
    let item_id = unique("itm");
    store
        .create_memory_item(&item(&item_id, &bank, "s"), "agent")
        .unwrap();
    let out = validate(&args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Attest,
        Some("api_key=sk-live-abcdefghij1234"),
        None,
        &unique("x"),
    ))
    .unwrap();
    let note = out.note.expect("note");
    assert!(
        !note.contains("sk-live-abcdefghij1234"),
        "note must be masked: {note}"
    );
}

#[test]
fn delete_is_a_documented_discard_alias() {
    assert_eq!(
        ValidateAction::parse("delete").unwrap(),
        ValidateAction::Discard
    );
    let store = store();
    let bank = unique("b");
    let item_id = unique("itm");
    store
        .create_memory_item(&item(&item_id, &bank, "s"), "agent")
        .unwrap();
    let out = validate(&args(
        &store,
        &bank,
        &item_id,
        ValidateAction::parse("delete").unwrap(),
        Some("ops removal"),
        None,
        &unique("x"),
    ))
    .unwrap();
    assert_eq!(out.action, "discard");
    assert!(out.applied);
    assert!(trail(&store, &bank, &item_id).contains(&"validate_discard".to_owned()));
}

#[test]
fn secret_reviewer_note_is_masked_before_durable_storage() {
    let store = store();
    let bank = unique("b");
    let item_id = unique("itm");
    store
        .create_memory_item(&item(&item_id, &bank, "s"), "agent")
        .unwrap();
    validate(&args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Invalidate,
        Some("api_key=sk-live-abcdefghij1234"),
        None,
        &unique("x"),
    ))
    .unwrap();
    let trail = store.audit_trail(&bank, &item_id).unwrap();
    let detail = trail
        .events
        .iter()
        .find(|e| e.operation == "validate_invalidate")
        .expect("validate_invalidate audit row")
        .detail
        .to_string();
    assert!(
        !detail.contains("sk-live-abcdefghij1234"),
        "durable audit detail must be masked: {detail}"
    );
}

#[test]
fn attribution_actor_must_be_known() {
    let store = store();
    let new_item_id = unique("x");
    let mut a = args(
        &store,
        "b",
        "itm",
        ValidateAction::Attest,
        None,
        None,
        &new_item_id,
    );
    a.actor = "not-an-actor";
    let err = validate(&a).unwrap_err();
    assert!(matches!(
        err.code,
        ErrorCode::InvalidArgument | ErrorCode::InvalidType
    ));
    // Known actor kinds parse.
    assert_eq!(Actor::parse("user").unwrap(), Actor::User);
}
