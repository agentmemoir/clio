// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the credential-bound conformance of the
//! administrative MCP mutations: `config_set`, `config_profile_apply`,
//! `ranking_env_set`, and a deployment-scoped `retention_profile_set`.
//!
//! ## Owns
//! - The assertion that an `agent` credential is refused with `forbidden`
//! - The assertion that a `harness` credential reaches the same mutations
//!
//! ## Does not own
//! - The credential store, server, and request helpers
//!   (`mcp_principal_binding_support`)
//! - The mutations themselves and their semantics (see `clio-mcp` and
//!   `clio-config`)
//!
//! ## Boundary
//!
//! Changes to which actor may mutate administrative configuration should
//! normally require changes here. Changes to the credential harness should not.
//!
//! Keep this module focused on the administrative authorization boundary.
//!
//! Run locally with `cargo test -p clio-mcp --test mcp_admin_authorization`.

// The shared harness carries helpers this target does not exercise; the
// existing target uses the rest, so the module as a whole is not dead.
#[allow(dead_code)]
mod mcp_principal_binding_support;

use clio_types::{Actor, ErrorCode};
use mcp_principal_binding_support::{
    Store, bearer, bound_server, call, post, tool_error_code, tool_ok,
};
use serde_json::{Value, json};

/// One call per administrative config/ranking mutation. The arguments succeed
/// for an administrator, so a refusal is the actor gate and not validation.
fn config_mutations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "config_set",
            json!({"path": "log.level", "value": "debug", "scope": "session"}),
        ),
        ("config_profile_apply", json!({"name": "coding_local"})),
        (
            "ranking_env_set",
            json!({
                "patch": {"retrieval": {"dense": 2.0, "lexical": 2.0, "importance": 2.0}},
                "dry_run": true,
            }),
        ),
    ]
}

/// The deployment-scoped retention mutation. A valid profile and `confirm=true`
/// reach the write, so a refusal is the actor gate and not a preview.
fn deployment_retention_mutation() -> Value {
    json!({
        "bank": "demo",
        "profile": {"verbosity": "selective"},
        "scope": "deployment",
        "confirm": true,
    })
}

#[test]
fn an_agent_credential_may_not_mutate_administrative_config() {
    let store = Store::new("admin-agent-config");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("bot", Actor::Agent, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let auth = bearer(&secrets[0]);

    for (name, args) in config_mutations() {
        let (status, response) = post(port, &call(name, args), &auth);
        assert_eq!(status, 200, "{name}: {response}");
        assert_eq!(
            tool_error_code(&response).as_deref(),
            Some(ErrorCode::Forbidden.as_str()),
            "an agent credential must be refused `{name}`: {response}"
        );
    }
}

#[test]
fn a_harness_credential_may_mutate_administrative_config() {
    let store = Store::new("admin-harness-config");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let auth = bearer(&secrets[0]);

    for (name, args) in config_mutations() {
        let (status, response) = post(port, &call(name, args), &auth);
        assert_eq!(status, 200, "{name}: {response}");
        assert!(
            tool_ok(&response),
            "a harness credential may `{name}`: {response}"
        );
    }
}

#[test]
fn a_system_credential_may_mutate_administrative_config() {
    let store = Store::new("admin-system-config");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("owner", Actor::System, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let auth = bearer(&secrets[0]);

    for (name, args) in config_mutations() {
        let (status, response) = post(port, &call(name, args), &auth);
        assert_eq!(status, 200, "{name}: {response}");
        assert!(
            tool_ok(&response),
            "a system credential may `{name}`: {response}"
        );
    }
    let (status, response) = post(
        port,
        &call("retention_profile_set", deployment_retention_mutation()),
        &auth,
    );
    assert_eq!(status, 200, "{response}");
    assert!(
        tool_ok(&response),
        "a system credential may set a deployment profile: {response}"
    );
}

#[test]
fn an_agent_credential_may_not_set_a_deployment_retention_profile() {
    let store = Store::new("admin-agent-retention");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("bot", Actor::Agent, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let auth = bearer(&secrets[0]);

    let (status, response) = post(
        port,
        &call("retention_profile_set", deployment_retention_mutation()),
        &auth,
    );
    assert_eq!(status, 200, "{response}");
    assert_eq!(
        tool_error_code(&response).as_deref(),
        Some(ErrorCode::Forbidden.as_str()),
        "an agent credential must be refused a deployment profile: {response}"
    );
}

#[test]
fn a_harness_credential_may_set_a_deployment_retention_profile() {
    let store = Store::new("admin-harness-retention");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let auth = bearer(&secrets[0]);

    let (status, response) = post(
        port,
        &call("retention_profile_set", deployment_retention_mutation()),
        &auth,
    );
    assert_eq!(status, 200, "{response}");
    assert!(
        tool_ok(&response),
        "a harness credential may set a deployment profile: {response}"
    );
}
