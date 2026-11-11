// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin the `tools/call` gate to
//! the tool view the server advertises: a name the effective selection leaves
//! out is refused with `-32602` naming it, and a name the selection carries
//! still dispatches.
//!
//! ## Owns
//! - A read-only selection refusing a write tool by name
//! - The `minimal` preset refusing a mutating tool while an advertised read
//!   tool still succeeds on the same state
//! - An installed group selection (what `--tools` writes) refusing a tool
//!   outside it
//!
//! ## Does not own
//! - What `tools/list` advertises (`tool_profile_tests`)
//! - The dispatched/listed/hidden parity across every catalog tool
//!   (`tool_profile_parity_tests`)
//! - The argument contract or the tool bodies (`arg_contract`, `write_tools`)
//!
//! ## Boundary
//!
//! Tests here break when the gate stops reading the visible view, or when the
//! selection stops reaching the state. They never change the gate, the profile
//! data, or a tool body.
//!
//! Keep this module focused on the gate's answer to a selection.

use crate::protocol::{ERR_INVALID_PARAMS, McpHandler};
use crate::runtime::{McpOpenOptions, McpState};
use clio_types::ToolGroup;
use serde_json::{Value, json};
use std::sync::Arc;

/// A state whose installed selection is `groups`, the seam a transport's
/// `--tools` writes before it opens, over the document `config`.
fn state_selected(config: &str, groups: Vec<ToolGroup>) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    runtime.set_mcp_tool_groups(groups);
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

/// A state whose configuration layer is exactly `config`.
fn state_with(config: &str) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

/// One `tools/call` response for `name`.
fn call(state: &Arc<McpState>, name: &str, args: &Value) -> Value {
    McpHandler::new(Arc::clone(state))
        .handle_message(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }))
        .expect("a request with an id has a response")
}

/// Assert the gate refused `name` with the unknown-tool shape and that no tool
/// body ran.
fn assert_refused_by_name(state: &Arc<McpState>, name: &str, args: &Value) {
    let response = call(state, name, args);
    assert!(
        response.get("result").is_none(),
        "`{name}` must be refused at the gate: {response}"
    );
    assert_eq!(
        response["error"]["code"],
        json!(ERR_INVALID_PARAMS),
        "{response}"
    );
    assert_eq!(
        response["error"]["message"],
        json!(format!("unknown tool `{name}`")),
        "{response}"
    );
    assert!(
        !state.calls.snapshot().contains_key(name),
        "a refused call must never reach a tool: {:?}",
        state.calls.snapshot()
    );
}

/// Assert the gate let `name` through to a successful tool body.
fn assert_dispatched(state: &Arc<McpState>, name: &str, args: &Value) {
    let response = call(state, name, args);
    assert!(
        response.get("error").is_none(),
        "`{name}` is advertised and must dispatch: {response}"
    );
    assert_eq!(
        response["result"]["structuredContent"]["ok"],
        json!(true),
        "`{name}` must succeed when the selection carries it: {response}"
    );
}

#[test]
fn a_read_only_selection_refuses_a_write_tool_by_name() {
    // The read-only selection: the minimal set plus `item-read`, which reaches
    // no group that writes. `store` belongs to `item-write`.
    let state = state_selected("{}", clio_types::custom(&["item-read"]).expect("a known group"));
    assert!(
        crate::bound_tools().contains(&"store"),
        "premise: this build binds the write tool the selection leaves out"
    );

    assert_refused_by_name(&state, "store", &json!({}));
    // The same state still answers a read tool it does advertise, so the
    // refusal above is the selection and not an unusable state.
    assert_dispatched(&state, "inspect", &json!({}));
}

#[test]
fn the_minimal_profile_refuses_a_mutating_tool_and_keeps_its_read_tools() {
    let state = state_with(r#"{"mcp": {"tool_profile": "minimal"}}"#);

    assert_refused_by_name(&state, "store", &json!({}));
    assert_refused_by_name(&state, "update", &json!({}));
    assert_dispatched(&state, "retrieve", &json!({"query": "preferred editor"}));
}

#[test]
fn a_group_selection_refuses_a_tool_outside_it() {
    // `--tools custom:ops` is the selection a transport installs; `store`
    // belongs to `item-write`, which `ops` and its closure never reach.
    let state = state_selected("{}", clio_types::custom(&["ops"]).expect("a known group"));
    assert!(
        !ToolGroup::Ops.requires().contains(&ToolGroup::ItemWrite),
        "premise: `ops` does not require the write group"
    );

    assert_refused_by_name(&state, "store", &json!({}));
    assert_dispatched(&state, "stats", &json!({}));
}
