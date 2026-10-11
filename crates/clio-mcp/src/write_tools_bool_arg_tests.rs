// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP binding's boolean argument reader: a missing or
//! null value takes the published default, and a present value of any other
//! JSON type is rejected with `invalid_type` instead of being silently
//! dropped.
//!
//! ## Owns
//! - `opt_bool` on absent, null, both booleans, and every wrong JSON type
//! - The tool-level error envelope for a wrong-typed toggle, `explain`
//!   argument, and confirm flag
//!
//! ## Does not own
//! - Which tools default each flag (`schema_read_defs`, `schema_write_defs`)
//! - The optional string, number, and context readers (`write_tools`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on boolean argument typing at the binding.

use super::*;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::json;
use std::sync::Arc;

/// Hermetic state; no call here depends on stored data.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: "bool-arg-tests".into(),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("state")
}

#[test]
fn opt_bool_reads_absent_null_and_both_booleans() {
    assert!(opt_bool(&json!({}), "flag", true).expect("absent uses the default"));
    assert!(!opt_bool(&json!({}), "flag", false).expect("absent uses the default"));
    assert!(opt_bool(&json!({"flag": null}), "flag", true).expect("null uses the default"));
    assert!(!opt_bool(&json!({"flag": false}), "flag", true).expect("false is read"));
    assert!(opt_bool(&json!({"flag": true}), "flag", false).expect("true is read"));
}

#[test]
fn opt_bool_rejects_every_wrong_json_type() {
    for wrong in [
        json!("false"),
        json!("true"),
        json!(0),
        json!(1.5),
        json!([]),
        json!({}),
    ] {
        let err = opt_bool(&json!({"flag": wrong.clone()}), "flag", false)
            .expect_err("a wrong type must fail");
        assert_eq!(err.code, ErrorCode::InvalidType, "{wrong}");
        assert_eq!(err.message, "flag must be a boolean", "{wrong}");
    }
}

#[test]
fn retrieve_rejects_a_string_boolean_toggle() {
    let state = state();
    for args in [
        json!({"query": "x", "include_entities": "false"}),
        json!({"query": "x", "entity_summary": "true"}),
        json!({"query": "x", "explain": 1}),
    ] {
        let out = dispatch(&state, "retrieve", &args);
        assert_eq!(out["ok"], json!(false), "{args}: {out}");
        assert_eq!(out["code"], json!("invalid_type"), "{args}: {out}");
    }
}

#[test]
fn retrieve_accepts_absent_and_null_booleans() {
    let state = state();
    let out = dispatch(&state, "retrieve", &json!({"query": "x"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let out = dispatch(
        &state,
        "retrieve",
        &json!({"query": "x", "include_entities": null}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
}

#[test]
fn discard_rejects_a_string_confirm_flag() {
    let state = state();
    let out = dispatch(
        &state,
        "discard",
        &json!({"item_id": "missing", "reason": "r", "confirm": "true"}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_type"), "{out}");
    assert_eq!(out["message"], json!("confirm must be a boolean"), "{out}");
}

#[test]
fn the_observe_writes_read_the_duplicate_waiver_as_a_strict_boolean() {
    // Both handlers build their admission policy from `allow_duplicate`, so `true`
    // is accepted and a value that is not a boolean is an error.
    let state = state();
    let cases = [
        (
            "persona_observe_preference",
            json!({"key": "verbosity", "observed_value": 0.5}),
        ),
        (
            "belief_observe",
            json!({
                "proposition": "p", "confidence": 0.5, "evidence_ref": "e",
                "source_type": "user_stated"
            }),
        ),
    ];
    for (tool, base) in cases {
        let mut waived = base.clone();
        waived["allow_duplicate"] = json!(true);
        let accepted = dispatch(&state, tool, &waived);
        assert_eq!(accepted["ok"], json!(true), "{tool}: {accepted}");
        let mut wrong = base;
        wrong["allow_duplicate"] = json!("yes");
        let refused = dispatch(&state, tool, &wrong);
        assert_eq!(refused["ok"], json!(false), "{tool}: {refused}");
        assert_eq!(refused["code"], json!("invalid_type"), "{tool}: {refused}");
    }
}
