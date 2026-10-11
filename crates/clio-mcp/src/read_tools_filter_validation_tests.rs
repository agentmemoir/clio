// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Regression tests for selectors that used to turn into empty inspect results.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, filter: Value) -> Value {
    dispatch(state, "inspect", &json!({"filter": filter}))
}

#[test]
fn malformed_inspect_selectors_fail_with_a_clear_error() {
    let s = state();
    for (filter, code) in [
        (json!({"category": "bogus"}), "invalid_argument"),
        (json!({"epistemic_kind": "maybe"}), "invalid_argument"),
        (json!({"time_start": "not-a-time"}), "invalid_argument"),
        (json!({"cateogry": "persona"}), "invalid_argument"),
        (json!({"category": 7}), "invalid_type"),
        (json!([]), "invalid_type"),
    ] {
        let result = call(&s, filter.clone());
        assert_eq!(result["ok"], json!(false), "filter {filter}: {result}");
        assert_eq!(result["code"], json!(code), "filter {filter}: {result}");
        assert!(
            result["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "filter {filter}: {result}"
        );
    }
}
