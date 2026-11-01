// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving that `sync_serve_with` applies the
//! caller's grant to the banks it serves, not only to the administrative gate:
//! the response reports the clamped bank list, and a `dev` start that names a
//! bank outside the grant is refused.
//!
//! ## Owns
//! - Serve-level coverage of the served-bank clamp and of the dev-mode refusal
//!
//! ## Does not own
//! - The clamp rule itself (`runtime_context`)
//! - The administrative gate and the serve argument shapes (`sync_tools_tests`)
//!
//! ## Boundary
//!
//! Tests here break when `sync_serve_with` stops consulting the grant; they
//! never change it.
//!
//! Keep this module focused on the grant where it crosses the serve boundary.

use super::sync_tools_test_support_tests::{TempDir, state};
use crate::principal::Principal;
use crate::runtime::McpState;
use serde_json::json;

/// A serving state bound to a `harness` credential whose grant is exactly `banks`.
fn serving(banks: &[&str]) -> McpState {
    state().with_principal(Principal::new(
        "serve-key".to_owned(),
        clio_types::Actor::Harness,
        banks.iter().map(|b| (*b).to_owned()).collect(),
    ))
}

/// A credential granted one bank asks to serve two: the response reports only
/// the bank it holds.
///
/// Outside dev mode the start needs auth material, so the scratch token store
/// and the scratch journal both live in this test's own directory, and the
/// one-second TTL ends the listener the assertion required.
#[test]
fn a_bound_serve_reports_the_clamped_bank_list() {
    let dir = TempDir::new("mcp-serve-clamp");
    let token_file = dir.key().with_file_name("auth-tokens.json");
    clio_config::auth_tokens::add_token(
        &token_file,
        "serve-clamp",
        "harness",
        &["bank-mcp".to_owned()],
    )
    .expect("the scratch credential store");
    let journal = dir.key().with_file_name("erasure-journal.json");
    let get = |name: &str| {
        (name == clio_config::ERASURE_JOURNAL_FILE_ENV).then(|| journal.display().to_string())
    };

    let out = super::sync_serve_with(
        &serving(&["bank-mcp"]),
        &json!({
            "token_file": token_file,
            "banks": ["bank-mcp", "bank-extra"],
            "ttl_secs": 1,
        }),
        &get,
    )
    .expect("the clamped start succeeds");

    assert_eq!(out["banks"], json!(["bank-mcp"]), "{out}");
    assert_eq!(out["dev"], json!(false), "{out}");
}

/// Dev mode lowers the auth requirement, never the grant: a start naming a bank
/// the credential does not hold is refused, and the refusal names neither the
/// bank nor the scope.
#[test]
fn dev_mode_refuses_a_serve_bank_outside_the_grant() {
    let dir = TempDir::new("mcp-serve-dev-grant");
    let journal = dir.key().with_file_name("erasure-journal.json");
    let get = |name: &str| {
        (name == clio_config::ERASURE_JOURNAL_FILE_ENV).then(|| journal.display().to_string())
    };
    let err = super::sync_serve_with(
        &serving(&["bank-mcp"]),
        &json!({ "dev": true, "banks": ["bank-extra"], "ttl_secs": 1 }),
        &get,
    )
    .expect_err("dev mode is not a grant");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden, "{err:?}");
    assert!(!err.message.contains("bank-extra"), "{err}");
}
