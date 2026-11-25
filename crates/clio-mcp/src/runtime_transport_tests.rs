// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the transport trust a state
//! carries: what the unbound allowance does on a listener that cannot vouch for
//! its caller, and that a trusted state keeps it.
//!
//! ## Owns
//! - The unbound administrative gate on a trusted state and on an untrusted one
//! - The credential rule a bound principal keeps wherever it is served
//! - The refusal text both answer with, asserted to name no secret and no path
//!
//! ## Does not own
//! - The actor resolution itself (see `runtime_context_transport_tests`)
//! - The transport that marks a state untrusted (see `http_post_tests`)
//!
//! ## Boundary
//!
//! Changes to what an unauthenticated off-machine listener allows should
//! normally require changes here. Changes to the administrative rule for a
//! bound credential should not.

use super::*;
use serde_json::json;

/// One in-memory state; each test builds its own, so no test depends on another.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state")
}

/// The same state as a listener reachable off this machine serves it.
fn untrusted() -> McpState {
    let mut state = state().as_ref().clone();
    state.mark_network_untrusted();
    state
}

/// A local, unbound state administers: the process owner already holds the store.
#[test]
fn a_trusted_unbound_state_administers() {
    state()
        .authorize_admin()
        .expect("a local, unbound state administers");
}

/// The same state behind an unauthenticated off-machine listener administers
/// nothing: nobody there can be shown to be the process owner.
#[test]
fn an_untrusted_unbound_state_refuses_to_administrate() {
    let err = untrusted()
        .authorize_admin()
        .expect_err("an unauthenticated off-machine listener must not administer");
    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert!(
        err.message.contains("unauthenticated"),
        "the refusal names the transport rule: {}",
        err.message
    );
}

/// A bound credential names its own actor, so the transport's trust decides
/// nothing about it: the administrative rule stays exactly the credential rule.
#[test]
fn a_bound_principal_keeps_the_credential_rule_on_an_untrusted_listener() {
    let mut bound = state().with_principal(Principal::new(
        "ops-key".to_owned(),
        Actor::Harness,
        vec!["default".to_owned()],
    ));
    bound.mark_network_untrusted();
    bound
        .authorize_admin()
        .expect("a harness credential administers wherever it is served");

    let mut non_admin = state().with_principal(Principal::new(
        "agent-key".to_owned(),
        Actor::Agent,
        vec!["default".to_owned()],
    ));
    non_admin.mark_network_untrusted();
    let err = non_admin
        .authorize_admin()
        .expect_err("a non-administrative credential must not administer");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("bundle import"), "{}", err.message);
}

/// The refusal a refused call actually carries is the shared message, and that
/// message names the transport while carrying no secret, no host, and no path.
#[test]
fn the_refusal_names_the_transport_and_nothing_else() {
    let message = untrusted()
        .require_authenticated_transport()
        .expect_err("an unbound untrusted state is refused")
        .message;
    assert_eq!(message, UNAUTHENTICATED_TRANSPORT_REFUSAL);
    assert!(message.contains("unauthenticated"), "{message}");
    for secret in ["token", "secret", "/", "\\", "localhost", "127.0.0.1"] {
        assert!(
            !message.contains(secret),
            "the refusal must not carry `{secret}`: {message}"
        );
    }
}

/// The transport gate answers on both branches: an untrusted unbound state is
/// refused, and a trusted one — or any bound one — passes.
#[test]
fn the_transport_gate_refuses_only_an_untrusted_unbound_state() {
    let untrusted_err = untrusted()
        .require_authenticated_transport()
        .expect_err("an unbound untrusted state is refused");
    assert_eq!(
        untrusted_err.code,
        ErrorCode::Forbidden,
        "{}",
        untrusted_err.message
    );

    state()
        .require_authenticated_transport()
        .expect("a trusted unbound state passes");

    let mut bound = state().with_principal(Principal::new(
        "ops-key".to_owned(),
        Actor::Harness,
        vec!["default".to_owned()],
    ));
    bound.mark_network_untrusted();
    bound
        .require_authenticated_transport()
        .expect("a bound principal is never refused by the transport rule");
}
