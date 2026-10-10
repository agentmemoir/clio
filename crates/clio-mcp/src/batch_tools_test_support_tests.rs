// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures used by the `batch` tool
//! MCP-surface tests: state construction, the batch call shim, a `store`
//! operation builder, and a bank row counter.
//!
//! ## Owns
//! - Batch test state builders and dispatch helpers
//!
//! ## Does not own
//! - The batch assertions themselves (see `batch_tools_tests` and
//!   `batch_tools_canonical_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn batch(state: &McpState, operations: Value, dry_run: bool) -> Value {
    crate::batch_tools::batch_tool(
        state,
        &json!({"operations": operations, "dry_run": dry_run}),
    )
}

pub(crate) fn store_op(id: &str, marker: &str) -> Value {
    json!({
        "tool": "store",
        "params": {
            "item": {
                "id": id,
                "subject_id": format!("subj-{id}"),
                "gist": format!("{marker} prefers terse answers"),
                "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
            },
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": format!("The user prefers terse answers ({marker}, 1 note since 2024-01-01)."),
            "bank": "bank-a",
            "actor": "agent"
        }
    })
}

pub(crate) fn count_bank(state: &McpState, bank: &str) -> usize {
    state
        .store
        .count_items(&clio_store::ItemInspectFilter::new(bank))
        .unwrap()
}
