// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the administrative gate on the
//! privileged subject-request binding: a bound credential that is not an
//! administrator is refused before the stored request is touched, and an
//! administrative credential reaches the same closure.
//!
//! ## Owns
//! - The bound-credential refusal and admission on `subject_request_close`
//!
//! ## Does not own
//! - The request sequencing and scope gate (`clio-compliance`)
//! - The other request-tool assertions (`subject_request_tools_tests`,
//!   `subject_request_tools_scenario_tests`)
//! - The credential model and its gate (`principal`, `runtime`)
//!
//! ## Boundary
//!
//! Changes to which credential may close a request should normally require
//! changes here. Changes to the credential gate itself should not.
//!
//! Test-only. Never compiled into production builds.

use crate::principal::Principal;
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_types::Actor;
use serde_json::{Value, json};

const BANK: &str = "subject-request-admin-bank";

fn state() -> std::sync::Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: BANK.into(),
            default_actor: Actor::Harness,
            ..McpOpenOptions::default()
        },
        &json!({ "mcp": { "tool_profile": "all" } }),
    )
    .expect("open state")
}

/// One copy of `state` bound as `actor` over the one bank this suite reaches.
fn bound_as(state: &McpState, actor: Actor) -> McpState {
    state.with_principal(Principal::new(
        format!("{}-key", actor.as_str()),
        actor,
        vec![BANK.to_owned()],
    ))
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// Open one request through the binding, so a closure has a stored record.
fn open_request(state: &McpState, request: &str) -> Value {
    call(
        state,
        "subject_request_start",
        json!({"bank": BANK, "kind": "erasure", "request_id": request}),
    )
}

/// The stored request as the read surface reports it.
fn status(state: &McpState, request: &str) -> Value {
    call(
        state,
        "subject_request_status",
        json!({"bank": BANK, "request_id": request}),
    )
}

#[test]
fn a_non_administrative_credential_may_not_close_a_request() {
    let s = state();
    let request = "req-admin-close";
    let started = open_request(&s, request);
    assert_eq!(started["ok"], json!(true), "{started}");
    let before = status(&s, request);
    for actor in [Actor::Agent, Actor::User] {
        let denied = call(
            &bound_as(&s, actor),
            "subject_request_close",
            json!({"bank": BANK, "request_id": request, "product_path": "erasure"}),
        );
        assert_eq!(denied["code"], json!("forbidden"), "{actor:?}: {denied}");
        assert!(
            denied.to_string().contains("not an administrator"),
            "{actor:?}: {denied}"
        );
    }
    assert_eq!(
        status(&s, request),
        before,
        "the stored request is unchanged"
    );
}

#[test]
fn an_administrative_credential_may_close_a_request() {
    let s = state();
    let admin = bound_as(&s, Actor::Harness);
    let request = "req-admin-close-ok";
    let started = open_request(&admin, request);
    assert_eq!(started["ok"], json!(true), "{started}");
    let completed = call(
        &admin,
        "subject_request_path_complete",
        json!({"bank": BANK, "request_id": request, "product_path": "erasure"}),
    );
    assert_eq!(completed["ok"], json!(true), "{completed}");
    let closed = call(
        &admin,
        "subject_request_close",
        json!({"bank": BANK, "request_id": request, "product_path": "erasure"}),
    );
    assert_eq!(closed["ok"], json!(true), "{closed}");
    assert_eq!(closed["request_outcome"], json!("completed"), "{closed}");
}
