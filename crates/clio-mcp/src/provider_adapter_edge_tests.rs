// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing provider adapter security, error handling,
//! pagination limits, and unimplemented provider stub responses.
//!
//! ## Owns
//! - Security tests (credential masking)
//! - Failure mode tests (auth failure)
//! - Contract tests (unimplemented provider stub)
//! - Pagination and cap tests for Mem0
//!
//! ## Does not own
//! - Core dry-run, pull, idempotency, or admission tests (`provider_adapter_tests.rs`)
//! - Transport implementation (`clio-index`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::provider_adapter_tests::{FakeMem0Server, call, state};
use serde_json::{Value, json};

#[test]
fn t05_planted_credential_masked_in_all_output() {
    let s = state();
    let secret = "sk-super-secret-token-987654321";
    let server = FakeMem0Server::spawn_json(json!([
        {
            "id": "sec-1",
            "memory": "User prefers Linux."
        }
    ]));

    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank",
            "actor": "harness",
            "credentials": {
                "api_key": secret
            },
            "options": {
                "api_base_url": server.url
            }
        }),
    );

    assert_eq!(out["ok"], Value::Bool(true));
    let out_text = out.to_string();
    assert!(
        !out_text.contains(secret),
        "secret was not masked: {out_text}"
    );

    let masked = out["credentials_masked"]["api_key"].as_str().unwrap();
    assert!(masked.starts_with("****"), "expected **** prefix: {masked}");
    assert!(masked.ends_with("4321"), "expected last 4: {masked}");
}

#[test]
fn t06_auth_failure_returns_structured_error() {
    let s = state();
    let server = FakeMem0Server::spawn_status(401, "{\"error\": \"Unauthorized API key\"}");

    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "bank": "test-bank",
            "actor": "harness",
            "credentials": {
                "api_key": "bad-key-12345"
            },
            "options": {
                "api_base_url": server.url
            }
        }),
    );

    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    let out_text = out.to_string();
    assert!(!out_text.contains("bad-key-12345"));
}

#[test]
fn t07_unimplemented_provider_returns_stub_code() {
    let s = state();
    for provider in ["hindsight", "mnemosyne", "honcho", "supermemory"] {
        let out = call(
            &s,
            "import_provider",
            json!({
                "provider": provider,
                "credentials": {"api_key": "sk-unimpl-key-123456"},
                "bank": "test-bank",
                "actor": "harness"
            }),
        );

        assert_eq!(
            out["ok"],
            Value::Bool(false),
            "failed for {provider}: {out}"
        );
        assert_eq!(out["code"], "PROVIDER_IMPORT_UNSUPPORTED");
        assert_eq!(out["writes_performed"], 0);
        assert!(!out.to_string().contains("sk-unimpl-key-123456"));
    }
}

#[test]
fn mem0_pagination_fetches_multiple_pages() {
    let s = state();
    let server = FakeMem0Server::spawn_handler(|path, _| {
        if path.contains("page=1") {
            (
                200,
                serde_json::to_vec(&json!([
                    {"id": "p-1", "memory": "Page 1 Item 1"},
                    {"id": "p-2", "memory": "Page 1 Item 2"}
                ]))
                .unwrap(),
            )
        } else if path.contains("page=2") {
            (
                200,
                serde_json::to_vec(&json!([
                    {"id": "p-3", "memory": "Page 2 Item 1"}
                ]))
                .unwrap(),
            )
        } else {
            (200, b"[]".to_vec())
        }
    });

    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": false,
            "bank": "test-bank",
            "actor": "harness",
            "options": {
                "api_base_url": server.url,
                "page_size": 2
            }
        }),
    );

    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["report"]["would_create"], 3);
    assert_eq!(out["writes_performed"], 3);

    assert!(
        s.store
            .get_memory_item("test-bank", "item-mem0-p-1")
            .unwrap()
            .is_some()
    );
    assert!(
        s.store
            .get_memory_item("test-bank", "item-mem0-p-3")
            .unwrap()
            .is_some()
    );
}

#[test]
fn mem0_category_inferences_and_options_override() {
    let s = state();
    let server = FakeMem0Server::spawn_json(json!([
        {"id": "cat-1", "memory": "Item 1", "categories": ["task-definition"]},
        {"id": "cat-2", "memory": "Item 2", "categories": ["schema-info"]},
        {"id": "cat-3", "memory": "Item 3", "categories": ["tool-usage"]},
        {"id": "cat-4", "memory": "Item 4", "categories": ["constraint-rule"]},
        {"id": "cat-5", "memory": "Item 5", "categories": ["user-preference"]},
        {"id": "cat-6", "memory": "Item 6", "category": "schema"}
    ]));

    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": false,
            "bank": "test-bank-cats",
            "actor": "harness",
            "options": { "api_base_url": server.url }
        }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["report"]["would_create"], 6);

    let item1 = s
        .store
        .get_memory_item("test-bank-cats", "item-mem0-cat-1")
        .unwrap()
        .unwrap();
    assert_eq!(item1.category, Some(clio_types::SemanticCategory::TaskSpec));
    let item2 = s
        .store
        .get_memory_item("test-bank-cats", "item-mem0-cat-2")
        .unwrap()
        .unwrap();
    assert_eq!(item2.category, Some(clio_types::SemanticCategory::Schema));
    let item3 = s
        .store
        .get_memory_item("test-bank-cats", "item-mem0-cat-3")
        .unwrap()
        .unwrap();
    assert_eq!(
        item3.category,
        Some(clio_types::SemanticCategory::ToolConfig)
    );
    let item4 = s
        .store
        .get_memory_item("test-bank-cats", "item-mem0-cat-4")
        .unwrap()
        .unwrap();
    assert_eq!(
        item4.category,
        Some(clio_types::SemanticCategory::OutputConstraint)
    );
    let item5 = s
        .store
        .get_memory_item("test-bank-cats", "item-mem0-cat-5")
        .unwrap()
        .unwrap();
    assert_eq!(item5.category, Some(clio_types::SemanticCategory::Persona));
    let item6 = s
        .store
        .get_memory_item("test-bank-cats", "item-mem0-cat-6")
        .unwrap()
        .unwrap();
    assert_eq!(item6.category, Some(clio_types::SemanticCategory::Schema));
}

#[test]
fn mem0_credentials_and_response_variations() {
    let s = state();
    let server = FakeMem0Server::spawn_json(json!({
        "results": [
            {"id": "res-1", "memory": "Wrapped in results object"}
        ]
    }));

    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-creds",
            "actor": "harness",
            "credentials": "Bearer my-secret-token",
            "options": {
                "api_base_url": server.url,
                "category": "persona"
            }
        }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["report"]["would_create"], 1);
    let auth = server.last_auth.lock().unwrap().clone();
    assert_eq!(auth, Some("Bearer my-secret-token".into()));

    for key_name in ["token", "key", "secret"] {
        let out_obj = call(
            &s,
            "import_provider",
            json!({
                "provider": "mem0",
                "dry_run": true,
                "bank": "test-bank-creds",
                "actor": "harness",
                "credentials": { key_name: "test-auth-val" },
                "options": { "api_base_url": server.url }
            }),
        );
        assert_eq!(out_obj["ok"], Value::Bool(true));
    }
}

#[test]
fn mem0_validation_rejections_and_limits() {
    let s = state();
    let server = FakeMem0Server::spawn_json(json!([
        {"memory": "Missing id field"},
        {"id": "no-text", "memory": "   "},
        {"id": "valid-1", "memory": "Valid item 1"},
        {"id": "valid-2", "memory": "Valid item 2"}
    ]));

    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-limits",
            "actor": "harness",
            "options": {
                "api_base_url": server.url,
                "max_items": 3,
                "agent_id": "agent-42"
            }
        }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["report"]["would_reject"], 2);
    assert_eq!(out["report"]["would_create"], 1);
    let notes = out["report"]["notes"].as_array().expect("notes");
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().unwrap().contains("reached max_items cap"))
    );

    let empty_server = FakeMem0Server::spawn_json(json!([]));
    let out_empty = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-limits",
            "actor": "harness",
            "options": { "api_base_url": empty_server.url }
        }),
    );
    assert_eq!(out_empty["ok"], Value::Bool(true));
    assert_eq!(out_empty["report"]["would_create"], 0);

    let bad_server = FakeMem0Server::spawn_status(200, "12345");
    let out_bad = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "bank": "test-bank-limits",
            "actor": "harness",
            "options": { "api_base_url": bad_server.url }
        }),
    );
    assert_eq!(out_bad["ok"], Value::Bool(false));
}

#[test]
fn mem0_retry_on_503_and_url_variations() {
    let s = state();
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts_clone = std::sync::Arc::clone(&attempts);

    let server = FakeMem0Server::spawn_handler(move |_, _| {
        let n = attempts_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            (503, b"{\"error\":\"Service Unavailable\"}".to_vec())
        } else {
            (
                200,
                serde_json::to_vec(&json!([{"id": "retried-1", "memory": "Ok after retry"}]))
                    .unwrap(),
            )
        }
    });

    let base_with_v1 = format!("{}/v1", server.url);
    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-retry",
            "actor": "harness",
            "options": { "api_base_url": base_with_v1 }
        }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["report"]["would_create"], 1);
    assert!(attempts.load(std::sync::atomic::Ordering::SeqCst) >= 2);

    let base_with_mem = format!("{}/v1/memories", server.url);
    let out_mem = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-retry",
            "actor": "harness",
            "options": { "api_base_url": base_with_mem }
        }),
    );
    assert_eq!(out_mem["ok"], Value::Bool(true), "{out_mem}");
}

#[test]
fn mem0_env_lookup_and_credential_mask_types() {
    let env_lookup = |k: &str| {
        if k == "CLIO_MEM0_API_KEY" {
            Some("env-secret-xyz9876".to_string())
        } else {
            None
        }
    };
    let masked = super::masked_credentials_with(&json!({}), &env_lookup);
    assert_eq!(masked["api_key"], "****9876");

    let extracted = super::extract_api_key_with(&json!({}), &env_lookup);
    assert_eq!(extracted.as_deref(), Some("env-secret-xyz9876"));

    let masked_str = super::masked_credentials(&json!({
        "credentials": "direct-secret-str-5432"
    }));
    assert_eq!(masked_str, "****5432");

    let masked_null = super::masked_credentials(&json!({
        "credentials": null
    }));
    assert_eq!(masked_null, Value::Null);

    let masked_num = super::masked_credentials(&json!({
        "credentials": 42
    }));
    assert_eq!(masked_num, json!(42));
}

/// The hard rename, pinned: the prefixed name is the only one read, and the bare
/// name resolves to nothing. A second spelling coming back is the failure this
/// test exists to catch.
#[test]
fn the_bare_provider_key_name_is_not_read() {
    let bare_only = |k: &str| (k == "MEM0_API_KEY").then(|| "bare-secret".to_owned());
    assert_eq!(super::extract_api_key_with(&json!({}), &bare_only), None);
    assert_eq!(
        super::masked_credentials_with(&json!({}), &bare_only),
        Value::Null
    );
}
