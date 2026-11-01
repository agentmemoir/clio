// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the sync surfaces beside the transfer tools
//! (`sync_status`, `sync_serve`) apply the same per-bank gate: a bound
//! credential's list must cover every bank the report or the served set
//! reaches, and an out-of-list bank is refused rather than dropped.
//!
//! ## Owns
//! - The status and serve refusals for a bank outside the credential's list
//!
//! ## Does not own
//! - The gate itself (`runtime_context`)
//! - The transfer tools' grant coverage (`sync_transfer_grant_tests`)
//! - The ack-skip bank grant (`sync_ack_tests`)
//! - The status and serve argument shapes (`sync_tools_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the sync surfaces' bank gate changes; they never
//! change it.
//!
//! Keep this module focused on the non-transfer sync tools' bank grant.

use super::{sync_serve_with, sync_status_with};
use crate::principal::Principal;
use crate::runtime::McpState;
use crate::sync_tools::sync_tools_test_support_tests::state;
use clio_types::ErrorCode;
use serde_json::json;

/// One state whose credential covers only `banks`.
fn bound(banks: &[&str]) -> McpState {
    state().with_principal(Principal::new(
        "sync-runner".into(),
        clio_types::Actor::Harness,
        banks.iter().map(|bank| (*bank).to_owned()).collect(),
    ))
}

/// The status report covers only banks the credential names: a foreign bank is
/// refused before any bank data or custody is read.
#[test]
fn status_refuses_a_bank_outside_the_grant() {
    let st = bound(&["b-allowed"]);
    let err = sync_status_with(
        &st,
        &json!({ "banks": ["b-foreign"], "sync_key": "k1" }),
        &|_| None,
    )
    .expect_err("a foreign bank is refused");
    assert_eq!(err.code, ErrorCode::Forbidden, "{err:?}");
    assert!(
        err.message.contains("outside this credential's banks"),
        "{err:?}"
    );
}

/// The served set covers only banks the credential names: a foreign bank is
/// refused before the server starts or the journal resolves.
#[test]
fn serve_refuses_a_bank_outside_the_grant() {
    let st = bound(&["b-allowed"]);
    let err = sync_serve_with(
        &st,
        &json!({ "dev": true, "ttl_secs": 1, "banks": ["b-foreign"] }),
        &|_| None,
    )
    .expect_err("a foreign bank is refused");
    assert_eq!(err.code, ErrorCode::Forbidden, "{err:?}");
}
