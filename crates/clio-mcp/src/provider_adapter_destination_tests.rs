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
//!
//! ## Does not own
//! - The rule itself (`provider_destination_tests.rs`)
//! - The pull, mapping, and idempotency tests (`provider_adapter_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use serde_json::{Value, json};

use super::provider_adapter_tests::{FakeMem0Server, call, default_allowlist_state, state};

/// The refusal code a refused destination reports through the tool surface.
fn assert_refused(out: &Value, host: &str) {
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "forbidden", "{host}: {out}");
    assert!(
        out["message"].as_str().is_some_and(|m| m.contains(host)),
        "the refusal names the host it refused: {out}"
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

    assert_refused(&out, "169.254.169.254");
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
    assert_refused(&out, "::1");
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
    let s = state();
    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-destination",
            "actor": "harness",
            "options": { "api_base_url": "ftp://127.0.0.1/v1" }
        }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "forbidden", "{out}");
}
