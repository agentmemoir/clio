// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Unit tests for the duplicate write CLI contract.

use super::{is_duplicate_noop, remember_refusal};
use serde_json::json;

#[test]
fn a_duplicate_payload_is_recognised() {
    assert!(is_duplicate_noop(&json!({"duplicate": true})));
}

#[test]
fn a_stored_payload_is_not_a_duplicate() {
    assert!(!is_duplicate_noop(&json!({"id": "itm-1", "pass": true})));
    assert!(!is_duplicate_noop(&json!({})));
}

#[test]
fn a_duplicate_is_refused_with_the_reason_named() {
    let refusal = remember_refusal(&json!({
        "duplicate": true,
        "rejection_reason": "exact_duplicate: identical text is already stored for `persona`",
    }))
    .expect("a duplicate is a refusal");
    assert_eq!(refusal.code, "duplicate");
    assert!(
        refusal.message.starts_with("duplicate: nothing stored"),
        "{}",
        refusal.message
    );
    assert!(
        refusal.message.contains("exact_duplicate"),
        "{}",
        refusal.message
    );
}

#[test]
fn a_duplicate_without_a_reason_falls_back_to_a_fixed_message() {
    let refusal = remember_refusal(&json!({"duplicate": true})).expect("a duplicate is a refusal");
    assert_eq!(
        refusal.message,
        "duplicate: nothing stored (identical text is already stored)"
    );
}

#[test]
fn a_stored_payload_is_not_refused_as_a_duplicate() {
    assert!(remember_refusal(&json!({"id": "itm-1", "pass": true})).is_none());
}
