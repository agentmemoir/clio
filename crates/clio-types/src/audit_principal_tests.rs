// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the principal label and the audited detail key it is
//! written under (`audit_principal.rs`).
//!
//! ## Owns
//! - Test coverage for label construction, display, and serialization
//! - Test coverage for the add-or-omit rule of `record_principal`
//!
//! ## Does not own
//! - The context field that carries the label (`context.rs`)
//! - Any audited write site
//!
//! ## Boundary
//!
//! Tests here break when the label or the audited key changes; they never
//! change it.
//!
//! Keep this module focused on the principal label's own tests.

use super::{PRINCIPAL_DETAIL_KEY, PrincipalLabel, record_principal};
use serde_json::{Value, json};

#[test]
fn label_exposes_its_text_and_serializes_as_a_string() {
    let label = PrincipalLabel::new("ci-runner");
    assert_eq!(label.as_str(), "ci-runner");
    assert_eq!(label.to_string(), "ci-runner");
    assert_eq!(
        serde_json::to_value(&label).expect("serialize label"),
        json!("ci-runner")
    );
    let back: PrincipalLabel =
        serde_json::from_value(json!("ci-runner")).expect("deserialize label");
    assert_eq!(back, label);
}

#[test]
fn record_principal_adds_the_key_to_an_object_detail() {
    let mut detail = json!({"result": "applied"});
    record_principal(&mut detail, Some(&PrincipalLabel::new("ops-key")));
    assert_eq!(
        detail,
        json!({"result": "applied", PRINCIPAL_DETAIL_KEY: "ops-key"})
    );
    assert_eq!(detail[PRINCIPAL_DETAIL_KEY], json!("ops-key"));
}

#[test]
fn record_principal_leaves_the_key_absent_without_a_principal() {
    let mut detail = json!({"result": "would_apply"});
    record_principal(&mut detail, None);
    assert_eq!(detail, json!({"result": "would_apply"}));
    assert!(
        detail.get(PRINCIPAL_DETAIL_KEY).is_none(),
        "an unbound operation must not carry an empty or null label"
    );
}

#[test]
fn record_principal_does_not_replace_a_non_object_detail() {
    for detail in [json!("plain"), json!([1, 2]), Value::Null] {
        let mut got = detail.clone();
        record_principal(&mut got, Some(&PrincipalLabel::new("ops-key")));
        assert_eq!(got, detail, "a non-object detail is left as it was");
    }
}

#[test]
fn record_principal_overwrites_a_stale_key_rather_than_duplicating_it() {
    let mut detail = json!({PRINCIPAL_DETAIL_KEY: "first"});
    record_principal(&mut detail, Some(&PrincipalLabel::new("second")));
    assert_eq!(detail, json!({PRINCIPAL_DETAIL_KEY: "second"}));
}
