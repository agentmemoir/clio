// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that a bound MCP caller's sync transfer tools stay inside
//! the credential's bank grant: an out-of-list bank is refused on push and on
//! pull, before any remote contact, a default bank outside the list is refused
//! the same way, a scoped list that excludes the default still runs and names
//! the credential on its response, and the shared-bank and unbound contracts
//! hold.
//!
//! ## Owns
//! - The push and pull refusals for a named bank outside the credential's list
//! - The refusal when the default bank resolves outside the list
//! - The successful scoped run whose list excludes the default bank, with the
//!   `principal` and masked token its response carries
//! - The shared gate on a named bank and the unbound actor/bank contract
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
use crate::runtime::{McpOpenOptions, McpState};
use crate::sync_tools::sync_tools_test_support_tests::{TempDir, state};
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

/// A scoped run succeeds when its list names only a granted bank: the default
/// bank is not checked, because the run never touches it. The pre-gate resolver
/// authorized the default and refused this run. The response names the
/// credential and echoes the token's last four characters only.
#[test]
fn a_scoped_list_runs_when_it_excludes_the_default_bank() {
    let st = bound(&["b-allowed"]);
    let dir = TempDir::new("mcp-scoped-grant");
    let key_path = dir.key().display().to_string();
    let journal_path = dir
        .key()
        .with_file_name("erasure-journal.json")
        .display()
        .to_string();
    let get = move |name: &str| match name {
        clio_config::SYNC_KEY_FILE_ENV => Some(key_path.clone()),
        clio_config::ERASURE_JOURNAL_FILE_ENV => Some(journal_path.clone()),
        _ => None,
    };
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("generate");
    let serve = crate::sync_tools::sync_serve_with(
        &st,
        &json!({ "dev": true, "ttl_secs": 5, "banks": ["b-allowed"] }),
        &get,
    )
    .expect("the peer starts serving");
    let addr = serve["addr"]
        .as_str()
        .expect("the bound address")
        .to_owned();
    let out = transfer_with(
        &st,
        &json!({ "remote": addr, "banks": ["b-allowed"], "token": "tok-abcdefghij" }),
        "push",
        &get,
    )
    .expect("a scoped push inside the credential's banks runs");
    assert_eq!(out["banks"][0]["bank_id"], "b-allowed");
    assert_eq!(out["principal"], "sync-runner");
    assert_eq!(out["token"], "***ghij");
}

/// Naming `shared` in the list does not bypass the deployment gate: the shared
/// namespace is refused while the deployment has not enabled it, even though
/// the credential names it.
#[test]
fn a_shared_bank_in_the_list_still_needs_the_deployment_gate() {
    let st = bound(&["bank-mcp", "shared"]);
    let err = transfer_with(&st, &json!({ "banks": ["shared"] }), "push", &|_| None)
        .expect_err("the shared gate refuses");
    assert_eq!(err.code, ErrorCode::Forbidden, "{err:?}");
    assert!(err.message.contains("shared"), "{err:?}");
}

/// Unbound local calls keep the argument contract: an unknown actor is
/// `invalid_argument`, a `shared` bank needs the deployment gate, and an empty
/// default bank is `missing_bank`.
#[test]
fn unbound_transfers_keep_the_actor_shared_and_missing_bank_contract() {
    let st = state();
    let no_key = |_name: &str| -> Option<String> { None };
    let actor = transfer_with(&st, &json!({ "actor": "robot" }), "push", &no_key)
        .expect_err("an unknown actor is refused");
    assert_eq!(actor.code, ErrorCode::InvalidArgument, "{actor:?}");
    let shared = transfer_with(&st, &json!({ "banks": ["shared"] }), "push", &no_key)
        .expect_err("the shared gate refuses");
    assert_eq!(shared.code, ErrorCode::Forbidden, "{shared:?}");
    assert!(shared.message.contains("shared"), "{shared:?}");
    let missing = transfer_with(&empty_bank_state(), &json!({}), "push", &no_key)
        .expect_err("an empty bank is refused");
    assert_eq!(missing.code, ErrorCode::MissingBank, "{missing:?}");
}

/// One state whose connection default bank is empty, so an omitted list
/// resolves to the empty bank the unbound contract refuses.
fn empty_bank_state() -> std::sync::Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: String::new(),
            default_actor: clio_types::Actor::Agent,
            shared_bank_enabled: false,
            kms_path: None,
        },
        &json!({}),
    )
    .expect("mcp state")
}
