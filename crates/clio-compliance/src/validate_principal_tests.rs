// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module pins what the presenting credential does to a durable reviewer
//! audit row: a bound action names the credential, a local one names nobody,
//! and the recorded label is text with nothing derived from a secret.
//!
//! ## Owns
//! - Test coverage for the credential key on a `validate_*` `audit_events` row
//! - Test coverage for its absence on an unbound row, and for the masking the
//!   reviewer note still gets
//!
//! ## Does not own
//! - Action semantics and note requirements (`validate_tests`)
//! - The reviewer routing itself (`validate.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the reviewer audit's attribution changes; they never
//! change it.
//!
//! Keep this module focused on the credential label on a reviewer audit row.

use super::validate_test_support::{args, bound_args, item, store};
use crate::test_unique::unique;
use crate::validate::{ValidateAction, validate};
use clio_store::{SqliteStore, Store};
use clio_types::{MemoryItem, PRINCIPAL_DETAIL_KEY, PrincipalLabel};
use serde_json::Value;

/// The `detail_json` of the one durable row a reviewer action left.
fn audit_detail(store: &SqliteStore, bank: &str, item_id: &str, operation: &str) -> Value {
    store
        .audit_trail(bank, item_id)
        .expect("trail")
        .events
        .into_iter()
        .find(|e| e.operation == operation)
        .unwrap_or_else(|| panic!("{operation} row present"))
        .detail
}

/// Seed one item and attest to it, then hand back its durable audit detail.
fn attest_detail(bank: &str, principal: Option<&PrincipalLabel>) -> Value {
    let store = store();
    let item_id = "itm-attested".to_owned();
    store
        .create_memory_item(
            &MemoryItem {
                bank_id: bank.to_owned(),
                ..item(&item_id, bank, "subj")
            },
            "agent",
        )
        .unwrap();
    validate(&bound_args(
        &store,
        bank,
        &item_id,
        ValidateAction::Attest,
        Some("checked"),
        None,
        "itm-new",
        principal,
    ))
    .unwrap();
    audit_detail(&store, bank, &item_id, "validate_attest")
}

#[test]
fn a_bound_attest_row_names_the_credential_beside_the_actor() {
    let bank = unique("b");
    let detail = attest_detail(&bank, Some(&PrincipalLabel::new("ops-key")));
    assert_eq!(detail[PRINCIPAL_DETAIL_KEY], "ops-key");
    assert_eq!(detail["action"], "attest");

    let store = store();
    let item_id = "itm-attested".to_owned();
    store
        .create_memory_item(
            &MemoryItem {
                bank_id: bank.clone(),
                ..item(&item_id, &bank, "subj")
            },
            "agent",
        )
        .unwrap();
    validate(&bound_args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Attest,
        Some("checked"),
        None,
        "itm-new",
        Some(&PrincipalLabel::new("ops-key")),
    ))
    .unwrap();
    let row = store.audit_trail(&bank, &item_id).unwrap();
    assert_eq!(
        row.events
            .iter()
            .find(|e| e.operation == "validate_attest")
            .expect("row")
            .actor,
        "harness",
        "the claimed role is unchanged"
    );
}

#[test]
fn a_local_attest_row_carries_no_credential_key() {
    let bank = unique("b");
    let store = store();
    let item_id = "itm-attested".to_owned();
    store
        .create_memory_item(
            &MemoryItem {
                bank_id: bank.clone(),
                ..item(&item_id, &bank, "subj")
            },
            "agent",
        )
        .unwrap();
    validate(&args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Attest,
        Some("checked"),
        None,
        "itm-new",
    ))
    .unwrap();
    let detail = audit_detail(&store, &bank, &item_id, "validate_attest");
    assert!(
        detail.get(PRINCIPAL_DETAIL_KEY).is_none(),
        "an unbound reviewer must not leave an empty or null label: {detail}"
    );
}

#[test]
fn a_bound_discard_row_names_the_credential() {
    let bank = unique("b");
    let store = store();
    let item_id = "itm-attested".to_owned();
    store
        .create_memory_item(
            &MemoryItem {
                bank_id: bank.clone(),
                ..item(&item_id, &bank, "subj")
            },
            "agent",
        )
        .unwrap();
    validate(&bound_args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Discard,
        Some("stale"),
        None,
        "itm-new",
        Some(&PrincipalLabel::new("ops-key")),
    ))
    .unwrap();
    assert_eq!(
        audit_detail(&store, &bank, &item_id, "validate_discard")[PRINCIPAL_DETAIL_KEY],
        "ops-key"
    );
}

#[test]
fn the_recorded_label_carries_no_secret_or_digest() {
    let bank = unique("b");
    let store = store();
    let item_id = "itm-attested".to_owned();
    store
        .create_memory_item(
            &MemoryItem {
                bank_id: bank.clone(),
                ..item(&item_id, &bank, "subj")
            },
            "agent",
        )
        .unwrap();
    validate(&bound_args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Attest,
        Some("api_key=sk-live-abcdefghij1234"),
        None,
        "itm-new",
        Some(&PrincipalLabel::new("ops-key")),
    ))
    .unwrap();
    let detail = audit_detail(&store, &bank, &item_id, "validate_attest").to_string();
    assert!(
        !detail.contains("sk-live-abcdefghij1234"),
        "the note is masked before it is recorded: {detail}"
    );
    assert!(
        detail.contains(PRINCIPAL_DETAIL_KEY) && detail.contains("ops-key"),
        "the label is recorded verbatim: {detail}"
    );
    assert!(
        !detail.contains("digest") && !detail.contains("sha256"),
        "the label is recorded as text and derives nothing from the secret: {detail}"
    );
}

#[test]
fn two_credentials_on_the_same_bank_leave_distinguishable_rows() {
    let bank = unique("b");
    let store = store();
    for (item_id, label) in [("itm-one", "ops-key"), ("itm-two", "review-key")] {
        store
            .create_memory_item(
                &MemoryItem {
                    bank_id: bank.clone(),
                    ..item(item_id, &bank, "subj")
                },
                "agent",
            )
            .unwrap();
        validate(&bound_args(
            &store,
            &bank,
            item_id,
            ValidateAction::Attest,
            Some("checked"),
            None,
            "itm-new",
            Some(&PrincipalLabel::new(label)),
        ))
        .unwrap();
        let detail = audit_detail(&store, &bank, item_id, "validate_attest");
        assert_eq!(detail[PRINCIPAL_DETAIL_KEY], label);
    }
}

/// The masked service view behind the audit read tool keeps the credential
/// key: masking defends secret-shaped values, and the label is none.
#[test]
fn the_masked_audit_trail_view_keeps_the_credential_key() {
    let bank = unique("b");
    let store = store();
    let item_id = "itm-attested".to_owned();
    store
        .create_memory_item(
            &MemoryItem {
                bank_id: bank.clone(),
                ..item(&item_id, &bank, "subj")
            },
            "agent",
        )
        .unwrap();
    validate(&bound_args(
        &store,
        &bank,
        &item_id,
        ValidateAction::Attest,
        Some("checked"),
        None,
        "itm-new",
        Some(&PrincipalLabel::new("ops-key")),
    ))
    .unwrap();
    let view = crate::audit::audit_trail(&store, &bank, &item_id).expect("view");
    let event = view
        .events
        .iter()
        .find(|e| e["operation"] == "validate_attest")
        .expect("attest event in the masked view");
    assert_eq!(event["detail"][PRINCIPAL_DETAIL_KEY], "ops-key");
    let timeline = view
        .timeline
        .iter()
        .find(|t| t["kind"] == "event" && t["entry"]["operation"] == "validate_attest")
        .expect("attest row in the merged timeline");
    assert_eq!(timeline["entry"]["detail"][PRINCIPAL_DETAIL_KEY], "ops-key");
}
