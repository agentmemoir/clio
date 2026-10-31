// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that a bound MCP caller's sync transfer tools stay inside
//! the credential's bank grant: an out-of-list bank is refused on push and on
//! pull, before any remote contact, and a default bank outside the list is
//! refused the same way.
//!
//! ## Owns
//! - The push and pull refusals for a named bank outside the credential's list
//! - The refusal when the default bank resolves outside the list
//!
//! ## Does not own
//! - The grant rule itself (`principal`, `runtime_context`)
//! - The transfer run and its denial guard (`sync_transfer_tests`)
//! - The argument shapes (`sync_tools_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the transfer tools' grant check changes; they never
//! change it.
//!
//! Keep this module focused on the transfer tools' bank grant.

use super::transfer_with;
use crate::principal::Principal;
use crate::runtime::McpState;
use crate::sync_tools::sync_tools_test_support_tests::state;
use clio_types::ErrorCode;
use serde_json::json;

/// One state whose credential covers only `banks`; the connection default stays
/// the support fixture's own, so a list that names it keeps resolving.
fn bound(banks: &[&str]) -> McpState {
    state().with_principal(Principal::new(
        "sync-runner".into(),
        clio_types::Actor::Harness,
        banks.iter().map(|bank| (*bank).to_owned()).collect(),
    ))
}

/// The refusal a bound caller gets for a bank its list does not name, with no
/// remote and no key: the grant check runs before either is read.
#[test]
fn push_refuses_an_out_of_list_bank() {
    let st = bound(&["bank-mcp"]);
    let err = transfer_with(&st, &json!({ "banks": ["bank-b"] }), "push", &|_| None)
        .expect_err("foreign bank refused on push");
    assert_eq!(err.code, ErrorCode::Forbidden, "{err:?}");
    assert!(
        err.message.contains("outside this credential's banks"),
        "{err:?}"
    );
}

/// The same refusal on the pull half, which applies remote content.
#[test]
fn pull_refuses_an_out_of_list_bank() {
    let st = bound(&["bank-mcp"]);
    let err = transfer_with(&st, &json!({ "banks": ["bank-b"] }), "pull", &|_| None)
        .expect_err("foreign bank refused on pull");
    assert_eq!(err.code, ErrorCode::Forbidden, "{err:?}");
    assert!(
        err.message.contains("outside this credential's banks"),
        "{err:?}"
    );
}

/// An omitted list resolves to the default bank, and a default outside the
/// credential's list is refused exactly like a named one.
#[test]
fn a_default_bank_outside_the_list_is_refused() {
    let st = bound(&["bank-a"]);
    let err = transfer_with(&st, &json!({}), "push", &|_| None)
        .expect_err("foreign default bank refused");
    assert_eq!(err.code, ErrorCode::Forbidden, "{err:?}");
}