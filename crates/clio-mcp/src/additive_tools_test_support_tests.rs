// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures used by the additive
//! tool MCP-surface tests: state construction (default, shared-enabled, and
//! rejecting) plus the write/read dispatch shims.
//!
//! ## Owns
//! - Additive test state builders and dispatch helpers
//!
//! ## Does not own
//! - The additive tool assertions themselves (see `additive_tools_tests` and
//!   `additive_tools_shared_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use clio_types::AmError;
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

pub(crate) fn shared_state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions {
        shared_bank_enabled: true,
        ..McpOpenOptions::default()
    })
    .expect("shared state")
}

/// State whose admission threshold rejects every candidate (θ > 1.0).
pub(crate) fn rejecting_state() -> Arc<McpState> {
    let mut state = McpState::open(&McpOpenOptions::default()).expect("state");
    Arc::get_mut(&mut state).expect("single owner").env = clio_config::ranking::RankingEnv {
        theta_admit: 1.1,
        ..clio_config::ranking::RankingEnv::system_defaults()
    };
    state
}

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn call(state: &McpState, name: &str, args: Value) -> Value {
    crate::write_tools::dispatch(state, name, &args)
}

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn call_read(state: &McpState, name: &str, args: Value) -> Result<Value, AmError> {
    crate::read_tools::dispatch(state, name, &args)
}

pub(crate) fn store_args(id: &str) -> Value {
    json!({
        "item": {
            "id": id,
            "subject_id": format!("subj-{id}"),
            "gist": "canonical scratchpad leak probe zqxj",
            "snapshot": {"entity": "probe", "amount": 1, "date": "2024-01-01"}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": "probe zqxj (1 note since 2024-01-01).",
        "bank": "bank-a",
        "actor": "agent"
    })
}
