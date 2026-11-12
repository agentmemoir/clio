// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin the `batch` op tool gate to
//! the tool view the server advertises: an operation naming a tool the active
//! selection hides is refused in band exactly as an operation naming a tool this
//! build does not bind is, and nothing the aborted batch staged is written.
//!
//! ## Owns
//! - A restricted selection refusing a hidden op tool by index, in band
//! - The hidden name and an unbound name answering one indistinguishable result
//! - An advertised op tool still committing under the same selection
//! - One op tool the default profile hides: refused when advertised, admitted
//!   when bound
//!
//! ## Does not own
//! - The dispatch gate on a direct `tools/call` (see
//!   `protocol_dispatch_gate_tests`)
//! - The batch engine's admission, `$ref`, and transaction semantics (see
//!   `batch_tools_tests`)
//! - What a tool accepts (see `arg_contract`)
//!
//! ## Boundary
//!
//! Tests here break when the op tool gate stops reading the visible view, or
//! when a hidden name starts answering differently from an unbound one. They
//! never change the gate, the group data, or a tool body.
//!
//! Keep this module focused on the gate's answer to a selection.

use super::batch_tools_test_support_tests::{batch, count_bank, store_op};
use crate::runtime::{McpOpenOptions, McpState};
use clio_types::ToolGroup;
use serde_json::json;
use std::sync::Arc;

/// A state whose installed selection is `groups`, the seam a transport's
/// `--tools` writes before it opens.
fn state_selected(groups: Vec<ToolGroup>) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime.set_mcp_tool_groups(groups);
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

/// Whether the state's advertised view carries `name`.
fn advertises(state: &McpState, name: &str) -> bool {
    crate::tools_page::visible_view(state)
        .expect("a readable view")
        .advertises(name)
}

/// The `custom:item-write` selection: `batch` and `store` advertised, the
/// persona, task, and triple op tools hidden.
fn item_write_state() -> Arc<McpState> {
    state_selected(clio_types::custom(&["item-write"]).expect("a known group"))
}

#[test]
fn a_hidden_op_tool_is_refused_and_writes_nothing() {
    let state = item_write_state();
    assert!(
        advertises(&state, "batch"),
        "premise: the selection advertises the batch tool"
    );
    assert!(
        advertises(&state, "store"),
        "premise: the selection advertises the `store` op tool"
    );
    assert!(
        !advertises(&state, "persona_put_stable"),
        "premise: the selection hides the `persona_put_stable` op tool"
    );

    // Op 0 would commit if the batch ran; op 1 names a hidden tool.
    let out = batch(
        &state,
        json!([
            store_op("itm-hidden-1", "alpha"),
            {"tool": "persona_put_stable", "params": {
                "key": "style", "value": "concise", "bank": "bank-a", "actor": "agent"
            }}
        ]),
        false,
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"), "{out}");
    assert_eq!(
        out["message"],
        json!("operation 1: unrecognized batch op tool `persona_put_stable`"),
        "{out}"
    );
    // The abort is atomic: op 0's staged row never committed.
    assert_eq!(count_bank(&state, "bank-a"), 0);
    assert!(
        state
            .store
            .get_memory_item("bank-a", "itm-hidden-1")
            .expect("read back")
            .is_none(),
        "a refused batch must leave no committed item"
    );
    assert!(state.leaves.get("bank-a", "itm-hidden-1").is_none());
}

#[test]
fn a_hidden_op_tool_reads_like_an_unbound_one() {
    let state = item_write_state();
    let hidden = batch(
        &state,
        json!([{"tool": "persona_put_stable", "params": {}}]),
        false,
    );
    let unbound = batch(
        &state,
        json!([{"tool": "no_such_tool", "params": {}}]),
        false,
    );

    assert_eq!(hidden["ok"], json!(false), "{hidden}");
    assert_eq!(unbound["ok"], json!(false), "{unbound}");
    assert_eq!(hidden["code"], unbound["code"], "{hidden} {unbound}");
    assert_eq!(
        hidden["message"],
        json!("operation 0: unrecognized batch op tool `persona_put_stable`"),
        "{hidden}"
    );
    assert_eq!(
        unbound["message"],
        json!("operation 0: unrecognized batch op tool `no_such_tool`"),
        "{unbound}"
    );
    // One envelope shape, so nothing but the name differs between them.
    let keys: Vec<&String> = hidden
        .as_object()
        .expect("a result object")
        .keys()
        .collect::<Vec<_>>();
    let unbound_keys: Vec<&String> = unbound
        .as_object()
        .expect("a result object")
        .keys()
        .collect::<Vec<_>>();
    assert_eq!(keys, unbound_keys, "{hidden} {unbound}");
}

#[test]
fn an_advertised_op_tool_still_commits_under_the_selection() {
    let state = item_write_state();
    let out = batch(&state, json!([store_op("itm-shown-1", "alpha")]), false);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["operations"][0]["status"], json!("COMMITTED"), "{out}");
    assert_eq!(count_bank(&state, "bank-a"), 1);
}

#[test]
fn a_read_only_selection_refuses_an_item_write_op_tool() {
    let state = state_selected(clio_types::custom(&["item-read"]).expect("a known group"));
    assert!(
        !advertises(&state, "store"),
        "premise: `item-read` closes no group that writes"
    );

    let out = batch(&state, json!([store_op("itm-read-only-1", "alpha")]), false);
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(
        out["message"],
        json!("operation 0: unrecognized batch op tool `store`"),
        "{out}"
    );
    assert_eq!(count_bank(&state, "bank-a"), 0);
    assert!(
        state
            .store
            .get_memory_item("bank-a", "itm-read-only-1")
            .expect("read back")
            .is_none(),
        "a refused batch must leave no committed item"
    );
}

#[test]
fn the_default_profile_refuses_an_op_tool_the_bound_scope_admits() {
    let s = McpState::open_with_effective(
        &McpOpenOptions::default(),
        &json!({"mcp": {"tool_profile": "minimal"}}),
    )
    .expect("state");
    assert!(
        super::BATCH_TOOLS.contains(&"store"),
        "premise: `store` is a batch op tool, so its refusal cannot come from the batch set"
    );
    assert!(
        !advertises(&s, "store"),
        "premise: the default profile hides the `store` op tool"
    );

    // The same operation, the same state, two scopes. Only the scope differs,
    // so a scope that stops hiding a name, or stops serving the bound
    // catalogue, fails one half.
    let operations = json!([store_op("itm-scope-1", "alpha")]);
    let refused = crate::batch_tools::batch_tool_in_scope(
        &s,
        &json!({"operations": operations}),
        crate::tools_page::ToolScope::Advertised,
    );
    assert_eq!(refused["ok"], json!(false), "{refused}");
    assert_eq!(refused["code"], json!("invalid_argument"), "{refused}");
    assert_eq!(
        refused["message"],
        json!("operation 0: unrecognized batch op tool `store`"),
        "{refused}"
    );
    assert_eq!(count_bank(&s, "bank-a"), 0);

    let admitted = crate::batch_tools::batch_tool_in_scope(
        &s,
        &json!({"operations": operations}),
        crate::tools_page::ToolScope::Bound,
    );
    assert_eq!(admitted["ok"], json!(true), "{admitted}");
    assert_eq!(
        admitted["operations"][0]["status"],
        json!("COMMITTED"),
        "{admitted}"
    );
    assert_eq!(count_bank(&s, "bank-a"), 1);
}
