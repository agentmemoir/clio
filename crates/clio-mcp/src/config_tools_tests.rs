// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the MCP config/ranking
//! surface: tool dispatch, secret masking in every view, validation failures,
//! the `ranking_env_set` dry-run path, and the published schema contract.
//!
//! ## Owns
//! - Assertions over the config/ranking tool results and published schemas
//! - The shared dispatcher helpers `call`, `ok`, and `code_of`
//!
//! ## Does not own
//! - Production config/ranking behavior (see `config_tools` and `clio-config`)
//! - Secret masking rules (`clio_config::secret`)
//! - The trust-reach liveness tests over the live retriever
//!   (`config_tools_trust_tests`)
//!
//! ## Boundary
//!
//! Changes to the config/ranking tool contract should normally require changes
//! here. Changes to config semantics should not.
//!
//! Keep this module focused on config/ranking surface tests.

use crate::runtime::{McpOpenOptions, McpState};
use serde_json::Value;
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

/// One MCP-level call through the real dispatcher (both sets route through
/// `write_tools::dispatch`, which forwards read names to the read surface).
#[allow(clippy::needless_pass_by_value)]
pub(super) fn call(state: &McpState, name: &str, args: Value) -> Value {
    crate::write_tools::dispatch(state, name, &args)
}

pub(super) fn ok(result: &Value) -> bool {
    result.get("ok").and_then(Value::as_bool) == Some(true)
}

pub(super) fn code_of(result: &Value) -> String {
    result
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_owned()
}

/// A planted secret never appears in plaintext in any config or
/// ranking view (`config_get`, `config_profiles`, `ranking_env_get`).
#[test]
fn planted_secret_is_masked_in_every_config_view() {
    let state = state();
    let secret = "sk-super-secret-value-9f2c";
    let set = call(
        &state,
        "config_set",
        serde_json::json!({"path": "credentials.extract_api_key", "value": secret, "scope": "session"}),
    );
    assert!(ok(&set), "{set}");
    let masked = set["value"].as_str().unwrap_or("?").to_owned();
    assert!(
        !masked.contains(secret),
        "config_set echoed the secret: {set}"
    );

    for (name, args) in [
        ("config_get", serde_json::json!({})),
        (
            "config_get",
            serde_json::json!({"path": "credentials.extract_api_key"}),
        ),
        ("config_profiles", serde_json::json!({})),
        ("ranking_env_get", serde_json::json!({})),
    ] {
        let view = call(&state, name, args);
        assert!(ok(&view), "{name}: {view}");
        assert!(
            !view.to_string().contains(secret),
            "{name} leaked the planted secret: {view}"
        );
    }
    // The masked view keeps the field with redacted content, not dropped.
    let path_view = call(
        &state,
        "config_get",
        serde_json::json!({"path": "credentials.extract_api_key"}),
    );
    assert_eq!(path_view["value"].as_str(), Some(masked.as_str()));
}

/// `config_set` applies at the session scope and rejects an invalid scope
/// before apply.
#[test]
fn config_set_applies_per_scope_and_rejects_bad_scope() {
    let state = state();
    let set = call(
        &state,
        "config_set",
        serde_json::json!({"path": "log.level", "value": "debug", "scope": "session"}),
    );
    assert!(ok(&set), "{set}");
    assert_eq!(set["scope"], serde_json::json!("session"));

    let get = call(
        &state,
        "config_get",
        serde_json::json!({"path": "log.level"}),
    );
    assert_eq!(get["value"], serde_json::json!("debug"));

    let bad = call(
        &state,
        "config_set",
        serde_json::json!({"path": "log.level", "value": "debug", "scope": "bogus"}),
    );
    assert!(!ok(&bad), "{bad}");
    assert_eq!(code_of(&bad), "invalid_argument");

    let profile_scope = call(
        &state,
        "config_set",
        serde_json::json!({"path": "log.level", "value": "info", "scope": "profile"}),
    );
    assert!(
        !ok(&profile_scope),
        "profile scope requires an active profile: {profile_scope}"
    );
}

/// An invalid value is rejected before apply and leaves the
/// effective view unchanged.
#[test]
fn config_set_rejects_unknown_path_and_invalid_value() {
    let state = state();
    let before = call(&state, "config_get", serde_json::json!({}));
    let bad_path = call(
        &state,
        "config_set",
        serde_json::json!({"path": "no.such.path", "value": 1}),
    );
    assert!(!ok(&bad_path), "{bad_path}");
    let bad_value = call(
        &state,
        "config_set",
        serde_json::json!({"path": "log.level", "value": "shout"}),
    );
    assert!(!ok(&bad_value), "{bad_value}");
    let after = call(&state, "config_get", serde_json::json!({}));
    assert_eq!(
        before, after,
        "a rejected set must not change the effective view"
    );
}

/// Profile apply returns the effective diff and the
/// known profile list; unknown profiles are rejected.
#[test]
fn config_profiles_list_and_apply() {
    let state = state();
    let profiles = call(&state, "config_profiles", serde_json::json!({}));
    assert!(ok(&profiles), "{profiles}");
    assert!(
        profiles["profiles"]
            .as_array()
            .expect("profiles")
            .iter()
            .any(|p| p["name"].as_str() == Some("coding_local")),
        "{profiles}"
    );

    let apply = call(
        &state,
        "config_profile_apply",
        serde_json::json!({"name": "coding_local"}),
    );
    assert!(ok(&apply), "{apply}");
    assert_eq!(apply["name"], serde_json::json!("coding_local"));
    assert!(
        apply.get("diff").is_some(),
        "profile apply must return the effective diff: {apply}"
    );

    let unknown = call(
        &state,
        "config_profile_apply",
        serde_json::json!({"name": "no-such-profile"}),
    );
    assert!(!ok(&unknown), "{unknown}");
    assert_eq!(code_of(&unknown), "unknown_profile");
}

/// `ranking_env_set` dry-run reports renormalized weights without
/// applying; a real patch is readable back through `ranking_env_get`.
#[test]
fn ranking_env_set_dry_run_and_apply() {
    let state = state();
    let dry = call(
        &state,
        "ranking_env_set",
        serde_json::json!({
            "patch": {"retrieval": {"dense": 2.0, "lexical": 2.0, "importance": 2.0}},
            "dry_run": true
        }),
    );
    assert!(ok(&dry), "{dry}");
    assert_eq!(dry["dry_run"], serde_json::json!(true));
    assert_eq!(dry["applied"], serde_json::json!(false));
    let weights = &dry["ranking"]["retrieval"];
    let sum: f64 = ["dense", "lexical", "importance"]
        .iter()
        .filter_map(|k| weights[*k].as_f64())
        .sum();
    assert!(
        (sum - 1.0).abs() < 1e-9,
        "weights must be renormalized: {dry}"
    );

    let apply = call(
        &state,
        "ranking_env_set",
        serde_json::json!({
            "patch": {"retrieval": {"dense": 0.4, "lexical": 0.3, "importance": 0.2}},
            "dry_run": false
        }),
    );
    assert!(ok(&apply), "{apply}");
    assert_eq!(apply["dry_run"], serde_json::json!(false));
    assert_eq!(apply["applied"], serde_json::json!(true));
    let after = call(&state, "ranking_env_get", serde_json::json!({}));
    let dense = after["retrieval"]["dense"].as_f64().expect("dense weight");
    // 0.4 + 0.3 + 0.2 = 0.9, so the applied dense weight is 0.4 of that sum.
    assert!(
        (dense - 0.4 / 0.9).abs() < 1e-9,
        "applied patch must be readable: {after}"
    );
    // The published payload carries exactly the three live weights, so no
    // consumer can read a retired key.
    let published = after["retrieval"].as_object().expect("retrieval object");
    assert_eq!(published.len(), 3, "three weights are published: {after}");
    assert!(published.get("temporal").is_none(), "{after}");
}

/// A malformed patch is rejected with a typed error before any apply.
#[test]
fn ranking_env_set_rejects_malformed_patch() {
    let state = state();
    let bad = call(
        &state,
        "ranking_env_set",
        serde_json::json!({"patch": {"retrieval": "not-an-object"}}),
    );
    assert!(!ok(&bad), "{bad}");
    assert_eq!(code_of(&bad), "invalid_type");
}

/// A patch still carrying a retired retrieval weight is rejected, and the
/// error names the key so the caller knows what to delete.
#[test]
fn ranking_env_set_rejects_a_retired_retrieval_weight() {
    let state = state();
    let bad = call(
        &state,
        "ranking_env_set",
        serde_json::json!({
            "patch": {"retrieval": {"dense": 0.4, "lexical": 0.3, "importance": 0.2, "temporal": 0.1}}
        }),
    );
    assert!(!ok(&bad), "{bad}");
    assert_eq!(code_of(&bad), "invalid_type", "{bad}");
    let message = bad["message"].as_str().expect("error message");
    // Assert on the shape only: the position suffix serde appends moves with
    // any reformatting of the payload.
    assert!(message.contains("unknown field `temporal`"), "{bad}");
    assert!(
        message.contains("expected one of `dense`, `lexical`, `importance`"),
        "{bad}"
    );
}

/// Unknown names never reach the config runtime: the dispatcher keeps the
/// `not_implemented` fallback.
#[test]
fn unknown_tool_names_fall_through() {
    let state = state();
    for name in ["config_delete", "ranking_env_reset"] {
        let result = call(&state, name, serde_json::json!({}));
        assert!(!ok(&result), "{name}: {result}");
        assert_eq!(code_of(&result), "not_implemented", "{result}");
    }
}

/// The published `config_set` schema states the omitted-scope default so a
/// schema-driven caller knows a missing `scope` means `session`.
#[test]
fn config_set_schema_states_session_default() {
    let pack = crate::schema::schema_pack();
    let tool = pack["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == serde_json::json!("config_set"))
        .expect("config_set published");
    assert_eq!(
        tool["inputSchema"]["properties"]["scope"]["default"],
        serde_json::json!("session"),
        "{tool}"
    );
}
