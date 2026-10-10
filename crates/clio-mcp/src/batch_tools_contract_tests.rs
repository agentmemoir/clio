// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that a `batch` operation passes the same argument contract as a
//! direct call: an undeclared argument aborts the batch before anything is written.
//!
//! ## Owns
//! - Assertions over `batch` operations that carry an undeclared argument
//!
//! ## Does not own
//! - The contract rules themselves (see `arg_contract_tests`)
//! - The batch's atomicity for gate refusals (see `batch_tools_tests`)
//!
//! ## Boundary
//!
//! Changes to how batch operations are validated should normally require changes
//! here. Changes to a single tool's arguments should not.
//!
//! Keep this module focused on the batch argument contract.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::json;
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

#[test]
fn a_batch_operation_with_an_undeclared_argument_aborts_and_writes_nothing() {
    let s = state();
    let out = dispatch(
        &s,
        "batch",
        &json!({
            "bank": "bank-a",
            "actor": "agent",
            "operations": [{
                "tool": "task_upsert",
                "params": {
                    "task_id": "t-batch", "definition": "run it", "status": "running",
                    "defintion": "typo"
                }
            }]
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    let message = out["message"].as_str().unwrap_or_default();
    assert!(message.contains("unknown argument `defintion`"), "{out}");
    let read = dispatch(
        &s,
        "task_get",
        &json!({"task_id": "t-batch", "bank": "bank-a", "actor": "agent"}),
    );
    assert_eq!(
        read["ok"],
        json!(false),
        "the aborted batch wrote nothing: {read}"
    );
}
