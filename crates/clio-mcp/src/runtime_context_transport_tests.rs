// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the actor rule of the per-call context helpers on a
//! listener that cannot vouch for its caller: which caller-declared actors such
//! a listener honours, and that a trusted one keeps honouring all of them.
//!
//! ## Owns
//! - `resolve_ctx` and `resolve_actor` on an untrusted state, for every actor
//!   value and for an absent declaration
//! - The assertion that the refusal is an authorization error rather than a
//!   panic, and that a non-privileged declaration still resolves
//!
//! ## Does not own
//! - The bound-versus-local label rule (see `runtime_context_tests`)
//! - The administrative gate on the same fact (see `runtime_transport_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the unbound actor rule changes; they never change it.

use super::*;
use crate::runtime::McpOpenOptions;
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

/// A privileged declaration is not honoured where nobody can be shown to be the
/// process owner.
#[test]
fn an_untrusted_listener_refuses_a_privileged_actor() {
    for actor in ["harness", "system"] {
        let ctx = untrusted()
            .resolve_ctx(&json!({"bank": "default", "actor": actor}))
            .expect_err("an unauthenticated off-machine listener refuses this");
        assert_eq!(ctx.code, ErrorCode::Forbidden, "{actor}: {}", ctx.message);
        assert!(ctx.message.contains("unauthenticated"), "{}", ctx.message);

        let resolved = untrusted().resolve_actor(&json!({"actor": actor}));
        let err = resolved.expect_err("the actor-only path refuses it too");
        assert_eq!(err.code, ErrorCode::Forbidden, "{actor}: {}", err.message);
    }
}

/// A non-privileged declaration carries no administrative reach, so the same
/// listener honours it exactly as a local one does.
#[test]
fn an_untrusted_listener_still_takes_an_unprivileged_actor() {
    for actor in [Actor::Agent, Actor::User] {
        let ctx = untrusted()
            .resolve_ctx(&json!({"bank": "default", "actor": actor.as_str()}))
            .expect("an unprivileged declaration is honoured");
        assert_eq!(ctx.actor, actor);
        let (resolved, label) = untrusted()
            .resolve_actor(&json!({"actor": actor.as_str()}))
            .expect("the actor-only path honours it too");
        assert_eq!(resolved, actor);
        assert!(label.is_none(), "an unbound state carries no label");
    }
}

/// A request that declares nothing uses the session default, so there is no
/// declaration to judge.
#[test]
fn an_untrusted_listener_resolves_an_absent_actor_to_the_default() {
    let ctx = untrusted()
        .resolve_ctx(&json!({"bank": "default"}))
        .expect("an absent declaration resolves");
    assert_eq!(ctx.actor, Actor::Agent, "the default actor resolves");
    let (resolved, _) = untrusted()
        .resolve_actor(&json!({}))
        .expect("the actor-only path resolves it too");
    assert_eq!(resolved, Actor::Agent);
}

/// The trusted listener keeps the old contract on both paths: a local process
/// owner may declare either administrative actor.
#[test]
fn a_trusted_listener_still_takes_a_privileged_actor() {
    for actor in [Actor::Harness, Actor::System] {
        let ctx = state()
            .resolve_ctx(&json!({"bank": "default", "actor": actor.as_str()}))
            .expect("a local transport honours the declaration");
        assert_eq!(ctx.actor, actor);
        let (resolved, _) = state()
            .resolve_actor(&json!({"actor": actor.as_str()}))
            .expect("the actor-only path honours it");
        assert_eq!(resolved, actor);
    }
}

/// An unknown actor value is still the argument error it always was: the
/// transport rule does not turn a typo into an authorization verdict.
#[test]
fn an_unknown_actor_value_stays_an_argument_error_on_both_transports() {
    for state in [state(), Arc::new(untrusted())] {
        let err = state
            .resolve_ctx(&json!({"bank": "default", "actor": "robot"}))
            .expect_err("an unknown actor is refused");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{}", err.message);
    }
}
