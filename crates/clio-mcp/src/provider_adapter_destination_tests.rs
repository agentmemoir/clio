// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the adapter's use of the destination
//! rule: a provider base URL the deployment was not given never reaches a
//! request, and one it was given still does.
//!
//! ## Owns
//! - The refusal a cloud-metadata, loopback, and unlisted host receives through
//!   the tool surface, asserting the error code rather than a private field
//! - The refusal happening before any request, proven by a zero request count
//! - The server key reaching an allowlisted destination and never a
//!   caller-chosen one, proven through the injected environment seam
//!
//! ## Does not own
//! - The rule itself (`provider_destination_tests.rs`)
//! - The credential decision in isolation (`provider_credentials_tests.rs`)
//! - The pull, mapping, and idempotency tests (`provider_adapter_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use super::provider_adapter_tests::{FakeMem0Server, call, default_allowlist_state, state};

/// The server key an injected environment reports, and nothing else.
const ENV_KEY: &str = "env-secret-xyz9876";

fn env(key: &str) -> Option<String> {
    (key == "CLIO_MEM0_API_KEY").then(|| ENV_KEY.to_owned())
}

/// The refusal code a refused destination reports through the tool surface.
fn assert_refused(out: &Value, host: &str) {
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "forbidden", "{host}: {out}");
    assert!(
        out["message"].as_str().is_some_and(|m| m.contains(host)),
        "the refusal names the host it refused: {out}"
    );
}

/// The refusal text a host refused by its address class receives, as opposed to
/// the one a host that is merely unlisted receives: an allowlist refusal names
/// the host too, so only the wording tells the two rules apart.
fn assert_refused_by_address_class(out: &Value, host: &str) {
    assert_refused(out, host);
    assert!(
        out["message"]
            .as_str()
            .is_some_and(|m| m.contains("non-public address")),
        "{host} must be refused for its address class rather than for being unlisted: {out}"
    );
}

#[test]
fn a_cloud_metadata_destination_is_refused_before_any_request() {
    // The fake server stands in for the caller's argument plumbing: nothing is
    // sent to the metadata address, and the request counter for this fixture
    // proves the refusal happened before the fetch.
    let s = default_allowlist_state(&["127.0.0.1"]);
    let server = FakeMem0Server::spawn_json(json!([]));
    let before = server.requests.load(std::sync::atomic::Ordering::SeqCst);

    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-destination",
            "actor": "harness",
            "options": { "api_base_url": "http://169.254.169.254/latest/meta-data/" }
        }),
    );

    assert_refused_by_address_class(&out, "169.254.169.254");
    assert_eq!(
        server.requests.load(std::sync::atomic::Ordering::SeqCst),
        before,
        "a refused destination must not be fetched"
    );
}

#[test]
fn a_loopback_destination_off_the_allowlist_is_refused() {
    let s = default_allowlist_state(&["127.0.0.1"]);
    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-destination",
            "actor": "harness",
            "options": { "api_base_url": "http://[::1]:9/v1", "base_url": "http://[::1]:9/v1" }
        }),
    );
    assert_refused_by_address_class(&out, "::1");
}

#[test]
fn an_unlisted_public_host_is_refused() {
    let s = default_allowlist_state(&["127.0.0.1"]);
    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-destination",
            "actor": "harness",
            "options": { "api_base_url": "https://attacker.example.com/v1" }
        }),
    );
    assert_refused(&out, "attacker.example.com");
}

#[test]
fn an_allowlisted_loopback_destination_still_reaches_the_provider() {
    // The fixture state allowlists 127.0.0.1, which is what an operator pointing
    // at a local provider would configure; it must keep working.
    let s = state();
    let server = FakeMem0Server::spawn_json(json!([
        {"id": "dest-1", "memory": "An allowlisted local provider stays reachable."}
    ]));
    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-destination",
            "actor": "harness",
            "options": { "api_base_url": server.url }
        }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["report"]["would_create"], 1);
    assert!(
        server.requests.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "an allowlisted destination is fetched"
    );
}

#[test]
fn a_non_http_scheme_is_refused() {
    // Each state's allowlist names the host its own URL carries, so the scheme
    // rule is the only rule left that can refuse it. With an allowlist of just
    // the shipped host every one of these is refused by the allowlist instead,
    // and the test would pass with the scheme check deleted.
    for (url, host) in [
        (
            "ftp://provider.example.invalid/v1",
            "provider.example.invalid",
        ),
        ("postgres://db.example.invalid/clio", "db.example.invalid"),
        ("file://api.example.invalid/v1", "api.example.invalid"),
    ] {
        let s = default_allowlist_state(&[host]);
        let out = call(
            &s,
            "import_provider",
            json!({
                "provider": "mem0",
                "dry_run": true,
                "bank": "test-bank-destination",
                "actor": "harness",
                "options": { "api_base_url": url }
            }),
        );
        assert_eq!(out["ok"], Value::Bool(false), "{url}: {out}");
        assert_eq!(out["code"], "forbidden", "{url}: {out}");
        let scheme = url.split_once("://").map_or("", |(scheme, _)| scheme);
        assert!(
            out["message"].as_str().is_some_and(
                |m| m.contains(&format!("must be http:// or https://, got `{scheme}://`"))
            ),
            "the refusal must name the scheme rule and the scheme it refused: {url}: {out}"
        );
    }
}

/// The tool surface, with the environment seam supplied: the shipped key can
/// only ever be observed through this injected lookup, so the suite never reads
/// or writes the process environment and cannot depend on the host.
#[allow(clippy::needless_pass_by_value)]
fn call_with_env(s: &crate::runtime::McpState, args: Value, lookup: super::EnvLookup<'_>) -> Value {
    match super::import_mem0(s, &args, lookup) {
        Ok(value) => value,
        Err(err) => json!({
            "ok": false,
            "code": err.code.as_str(),
            "message": err.message
        }),
    }
}

/// A state whose allowlist names only the shipped host, so the fake server's
/// address is a destination the operator never sanctioned: the request counter
/// then genuinely proves nothing was fetched.
fn shipped_host_only() -> std::sync::Arc<crate::runtime::McpState> {
    default_allowlist_state(&[clio_types::BUILT_IN_PROVIDER_HOST])
}

#[test]
fn an_unlisted_endpoint_with_no_credential_never_receives_the_server_key() {
    let s = shipped_host_only();
    let server = FakeMem0Server::spawn_json(json!([{"id": "never", "memory": "never"}]));
    let before = server.requests.load(Ordering::SeqCst);

    let out = call_with_env(
        &s,
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-credentials",
            "actor": "harness",
            "options": { "api_base_url": server.url }
        }),
        &env,
    );

    assert_refused(&out, "127.0.0.1");
    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        before,
        "a refused destination must not be fetched"
    );
    assert!(
        server.last_auth.lock().is_ok_and(|a| a.is_none()),
        "no request was recorded, so no credential was offered"
    );
}

#[test]
fn an_unlisted_endpoint_with_caller_credentials_is_still_refused() {
    let s = shipped_host_only();
    let server = FakeMem0Server::spawn_json(json!([{"id": "never", "memory": "never"}]));
    let before = server.requests.load(Ordering::SeqCst);

    let out = call_with_env(
        &s,
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-credentials",
            "actor": "harness",
            "credentials": { "api_key": "caller-supplied-key-1234" },
            "options": { "api_base_url": server.url }
        }),
        &env,
    );

    assert_refused(&out, "127.0.0.1");
    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        before,
        "the destination rule does not depend on which credential was supplied"
    );
}

#[test]
fn an_unlisted_public_host_with_no_credential_is_refused() {
    let s = shipped_host_only();
    let out = call_with_env(
        &s,
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-credentials",
            "actor": "harness",
            "options": { "api_base_url": "https://attacker.example.com/v1" }
        }),
        &env,
    );

    assert_refused(&out, "attacker.example.com");
}

#[test]
fn an_allowlisted_endpoint_with_no_caller_credential_uses_the_server_key() {
    let s = state();
    let server = FakeMem0Server::spawn_json(json!([
        {"id": "cred-1", "memory": "The shipped path keeps using the server key."}
    ]));

    let out = call_with_env(
        &s,
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-credentials",
            "actor": "harness",
            "options": { "api_base_url": server.url }
        }),
        &env,
    );

    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["report"]["would_create"], 1);
    let auth = server.last_auth.lock().expect("auth lock").clone();
    assert_eq!(
        auth.as_deref(),
        Some("Token env-secret-xyz9876"),
        "the server key is offered to an allowlisted destination"
    );
    assert_eq!(
        out["credentials_masked"],
        json!({"api_key": "[REDACTED]"}),
        "the report names the credential that was sent, redacted"
    );
}

#[test]
fn a_report_claims_no_credential_for_an_unlisted_endpoint() {
    let masked = crate::provider_credentials::masked_credentials;
    assert_eq!(
        masked(&json!({ "credentials": null }), &env, false),
        Value::Null
    );
    assert_eq!(masked(&json!({}), &env, false), Value::Null);
    assert_eq!(
        masked(&json!({}), &env, true),
        json!({"api_key": "[REDACTED]"})
    );
}
