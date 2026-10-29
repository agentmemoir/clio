// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the inspect payload's `include_discarded` echo
//! reports the filter the listing actually applied, not just the flag.
//!
//! ## Owns
//! - The echo for the default listing, the explicit flag, and a
//!   `validity: "discarded"` selection
//! - The discarded rows returned by the `validity: "discarded"` selection
//!
//! ## Does not own
//! - The inspect filter parsing and the store clauses (`read_tools_tests`,
//!   the store inspect query)
//! - The discard write path (`read_tools_lifecycle_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the echoed filter state.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// Store two items in one bank and discard the first through the real path.
fn bank_with_one_discarded_item(state: &McpState) {
    for (id, text) in [
        ("itm-echo-live", "echo live note"),
        ("itm-echo-gone", "echo gone note"),
    ] {
        let out = call(
            state,
            "store",
            json!({
                "item": {
                    "id": id,
                    "subject_id": "subj",
                    "gist": text,
                    "snapshot": {"entity": text, "amount": 1, "date": "2024-01-01"}
                },
                "category": "task_spec",
                "epistemic_kind": "fact",
                "source_text": format!("The user prefers {text} (1 note since 2024-01-01)."),
                "bank": "bank-a",
                "actor": "agent",
                "timestamp": "2024-01-01T00:00:00Z"
            }),
        );
        assert_eq!(out["ok"], json!(true), "store {id}: {out}");
    }
    let discarded = call(
        state,
        "discard",
        json!({
            "bank": "bank-a",
            "item_id": "itm-echo-gone",
            "reason": "ops",
            "confirm": true,
            "timestamp": "2024-02-01T00:00:00Z"
        }),
    );
    assert_eq!(discarded["ok"], json!(true), "{discarded}");
}

#[test]
fn validity_discarded_echoes_true_and_returns_discarded_rows() {
    let s = state();
    bank_with_one_discarded_item(&s);

    let out = call(
        &s,
        "inspect",
        json!({"bank": "bank-a", "filter": {"validity": "discarded"}}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["include_discarded"], json!(true), "{out}");
    let items = out["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{out}");
    assert_eq!(items[0]["item_id"], json!("itm-echo-gone"));
}

#[test]
fn explicit_include_discarded_echoes_true() {
    let s = state();
    bank_with_one_discarded_item(&s);

    let out = call(
        &s,
        "inspect",
        json!({"bank": "bank-a", "filter": {"include_discarded": true}}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["include_discarded"], json!(true), "{out}");
}

#[test]
fn default_listing_echoes_false() {
    let s = state();
    bank_with_one_discarded_item(&s);

    let out = call(&s, "inspect", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["include_discarded"], json!(false), "{out}");
    assert_eq!(out["items"].as_array().expect("items").len(), 1, "{out}");
}
