// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the operator's `sync.max_envelope_skew_secs` setting
//! reaches the sync client and server options: the configured value, the
//! default when the key is absent, and the value `0`, which disables the
//! window and is not a missing value.
//!
//! ## Owns
//! - Test coverage for `McpState::sync_max_envelope_skew_secs`
//! - Test coverage for the freshness window in `client_cfg`
//!
//! ## Does not own
//! - The freshness guard itself (`clio-sync` apply)
//! - The config document and its validation (`clio-config`)
//!
//! ## Boundary
//!
//! Tests here break when the setting's wiring changes; they never change it.
//!
//! Keep this module focused on the setting's route to the sync options.

use crate::runtime::{McpOpenOptions, McpState};
use crate::sync_tools::client_cfg;
use serde_json::json;
use std::sync::Arc;

/// A state whose config layers `document` over the deployment defaults, so a
/// value no environment variable can spell is reachable without touching the
/// process environment.
fn state_with_document(document: &serde_json::Value) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_| None);
    runtime
        .load_file_json(&document.to_string())
        .expect("config");
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

#[test]
fn the_freshness_window_reaches_the_client_config() {
    let state = state_with_document(&json!({"sync": {"max_envelope_skew_secs": 42}}));
    assert_eq!(
        client_cfg(&state, None, None).max_envelope_skew_secs,
        42,
        "the configured window must reach the client it bounds"
    );
}

#[test]
fn the_freshness_window_defaults_to_the_shipped_window() {
    let state = state_with_document(&json!({}));
    assert_eq!(
        state.sync_max_envelope_skew_secs(),
        clio_sync::ENVELOPE_SKEW_DEFAULT_SECS,
        "a deployment that never set the key uses the default window"
    );
    assert_eq!(
        client_cfg(&state, None, None).max_envelope_skew_secs,
        clio_sync::ENVELOPE_SKEW_DEFAULT_SECS,
        "the default reaches the client config too"
    );
}

#[test]
fn a_zero_freshness_window_survives_the_round_trip() {
    let state = state_with_document(&json!({"sync": {"max_envelope_skew_secs": 0}}));
    assert_eq!(
        state.sync_max_envelope_skew_secs(),
        0,
        "zero disables the window and must not be replaced by the default"
    );
    assert_eq!(
        client_cfg(&state, None, None).max_envelope_skew_secs,
        0,
        "the disabled window reaches the client config as zero"
    );
}

#[test]
fn a_wrong_typed_freshness_window_falls_back_to_the_default() {
    let state = state_with_document(&json!({"sync": {"max_envelope_skew_secs": "soon"}}));
    assert_eq!(
        state.sync_max_envelope_skew_secs(),
        clio_sync::ENVELOPE_SKEW_DEFAULT_SECS,
        "a value that is not a count cannot open the window"
    );
}
