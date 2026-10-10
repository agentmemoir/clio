// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the batch preview's refusal: a preview where every item fails
//! validation is an error, and a preview with one usable item keeps its per-item output.
//!
//! ## Owns
//! - Assertions over `admit_preview_batch` for all-invalid and mixed batches
//!
//! ## Does not own
//! - The admission math for one item (see `store_write_tests`)
//!
//! ## Boundary
//!
//! Changes to the refusal rule should normally require changes here. Changes to one
//! item's gates should not require changes here.
//!
//! Keep this module focused on the batch preview refusal.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};

fn state() -> std::sync::Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn preview(state: &McpState, items: Value) -> Value {
    dispatch(
        state,
        "admit_preview_batch",
        &json!({"items": items, "bank": "bank-a", "actor": "agent"}),
    )
}

#[test]
fn a_batch_where_every_item_fails_validation_is_refused() {
    let s = state();
    let out = preview(
        &s,
        json!([
            {"category": "persona", "epistemic_kind": "fact"},
            {"item": {"gist": "   "}, "category": "persona", "epistemic_kind": "fact"}
        ]),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"), "{out}");
    let message = out["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(
            "no item previewed: all 2 items failed validation; item 0: item.gist is required"
        ),
        "{out}"
    );
}

#[test]
fn a_batch_with_one_usable_item_keeps_its_per_item_output() {
    let s = state();
    let out = preview(
        &s,
        json!([
            {"item": {"gist": "terse batch answers"}, "category": "persona", "epistemic_kind": "fact"},
            {"category": "persona", "epistemic_kind": "fact"}
        ]),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["count"], json!(2), "{out}");
    assert!(out["items"][1]["error"].is_object(), "{out}");
}
