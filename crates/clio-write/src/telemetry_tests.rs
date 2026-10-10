// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for verify-telemetry event tests: the sink
//! records every event, the principal key follows the bound/local rule, and
//! preview masking keeps secrets out of the recorded preview.
//!
//! ## Owns
//! - `VerifyEvent` serialization shape (key absent when unbound, label text
//!   when bound)
//! - `redact_preview` masking behaviour assertions
//!
//! ## Does not own
//! - The event struct and masking implementation (`telemetry.rs`)
//! - Durable store-path coverage (`store_path_tests`)
//!
//! ## Boundary
//!
//! Tests here break when telemetry serialization or masking changes; they
//! never change it.
//!
//! Keep this module focused on verify-telemetry event behaviour.

use super::{VerifyEvent, VerifySink, VerifyVecSink, redact_preview};
use serde_json::json;

#[test]
fn sink_records_events() {
    let sink = VerifyVecSink::default();
    sink.emit(VerifyEvent {
        operation: "verify_ok".into(),
        bank: "b".into(),
        actor: "agent".into(),
        principal: None,
        source_ref: "t".into(),
        timestamp: "t0".into(),
        failure_fields: vec![],
        preview: json!({}),
    });
    assert_eq!(sink.events().len(), 1);
}

#[test]
fn an_absent_principal_is_omitted_from_the_serialized_event() {
    let event = VerifyEvent {
        operation: "verify_ok".into(),
        bank: "b".into(),
        actor: "agent".into(),
        principal: None,
        source_ref: "t".into(),
        timestamp: "t0".into(),
        failure_fields: vec![],
        preview: json!({}),
    };
    let json = serde_json::to_value(&event).expect("serialize event");
    assert!(
        json.get("principal").is_none(),
        "an unbound event carries no key at all, never a null: {json}"
    );
    assert_eq!(json["actor"], "agent", "existing fields are unchanged");
}

#[test]
fn a_present_principal_is_recorded_as_its_label_text() {
    let event = VerifyEvent {
        operation: "verify_ok".into(),
        bank: "b".into(),
        actor: "agent".into(),
        principal: Some("ops-key".into()),
        source_ref: "t".into(),
        timestamp: "t0".into(),
        failure_fields: vec![],
        preview: json!({}),
    };
    let json = serde_json::to_value(&event).expect("serialize event");
    assert_eq!(json["principal"], "ops-key");
    // The label is the operator-facing name, recorded verbatim: no digest of a
    // secret can appear because nothing is derived from one.
    let back: VerifyEvent = serde_json::from_value(json).expect("deserialize event");
    assert_eq!(back.principal.as_deref(), Some("ops-key"));
}

#[test]
fn redact_masks_api_key_in_gist_and_json() {
    let preview = redact_preview(
        Some(&json!({"api_key": "sk-SECRETKEY99", "entity": "Ada"})),
        Some("note api_key: sk-SECRETKEY99 and api_key: sk-OTHERKEY77 end"),
        Some(&json!({"token": "abcdEFGH1234"})),
    );
    assert_eq!(preview["snapshot"]["****_key"], json!("****EY99"));
    assert_eq!(preview["snapshot"]["entity"], json!("Ada"));
    let gist = preview["gist"].as_str().unwrap();
    assert!(!gist.contains("sk-SECRETKEY99"));
    assert!(!gist.contains("sk-OTHERKEY77"));
    assert!(gist.contains("****"));
    assert_eq!(preview["diagnostic"]["****oken"], json!("****1234"));
}
