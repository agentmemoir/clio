// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP-bound sync tools (`sync_tools.rs`): dispatch
//! routing, argument validation, the masked `sync_status` report shape, the
//! `sync_serve` removed-token rejection, dev-mode refusal and ephemeral-bind
//! success shape, and schema publication.
//!
//! ## Owns
//! - Test coverage for sync tool dispatch, argument validation, and response
//!   shapes over an in-memory sqlite fixture
//!
//! ## Does not own
//! - The tool behavior itself (`sync_tools.rs` or `clio-sync`)
//! - The `sync_status` `sync_key` override and key custody tests (see
//!   `sync_tools_key_tests`)
//! - Sync protocol/apply semantics tests (`clio-sync` crate tests)
//!
//! ## Boundary
//!
//! Tests here break when MCP sync dispatch changes; they never change it.
//!
//! Keep this module focused on MCP sync tool tests.

use super::sync_tools_test_support_tests::{TempDir, state};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::path::PathBuf;

/// One scratch credential store, removed on drop. The repo's own fixture
/// pattern, as in `http_auth_tests::Scratch`: `tempfile` is not a workspace
/// dependency, so the pid and a nanosecond stamp keep parallel tests apart.
struct ScratchStore {
    path: PathBuf,
}

impl ScratchStore {
    fn new(label: &str, secret: &str, banks: &[&str]) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "am-mcp-serve-{label}-{}-{nanos}.json",
            std::process::id()
        ));
        let record = clio_config::auth_tokens::AuthTokenRecord {
            label: label.to_owned(),
            secret_sha256: clio_config::auth_tokens::digest_hex(secret),
            actor: clio_types::Actor::Agent,
            banks: banks.iter().map(|b| (*b).to_owned()).collect(),
            created_at: "2026-10-01T00:00:00Z".into(),
        };
        clio_config::auth_tokens::save_auth_tokens(&path, &[record]).expect("save the store");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("tighten");
        }
        Self { path }
    }

    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}

impl Drop for ScratchStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The three transfer modes run end to end against this node's own dev serve:
/// the push echoes a masked token, the pull and the bidirectional run come
/// back with a mode, a remote, and a cursor per bank.
///
/// It drives `transfer_with` rather than the env-reading `transfer` wrapper so
/// the sync key and the denial journal come from the injected getter: a
/// parallel suite must not mutate the process environment, and a keyless
/// transfer is refused.
#[test]
fn sync_transfer_roundtrip_against_local_dev_serve() {
    let st = state();
    let serve = dispatch(&st, "sync_serve", &json!({ "dev": true, "ttl_secs": 20 }));
    assert_eq!(serve["ok"], Value::Bool(true), "{serve}");
    let addr = serve["addr"].as_str().expect("addr").to_owned();

    let dir = TempDir::new("mcp-transfer-roundtrip");
    let key_path = dir.key().display().to_string();
    let journal_path = dir
        .key()
        .with_file_name("erasure-journal.json")
        .display()
        .to_string();
    let key_only = |name: &str| (name == clio_config::SYNC_KEY_FILE_ENV).then(|| key_path.clone());
    clio_config::sync_key::generate(&key_only, Some(clio_config::KeySource::File))
        .expect("generate");
    let get = move |name: &str| match name {
        clio_config::SYNC_KEY_FILE_ENV => Some(key_path.clone()),
        clio_config::ERASURE_JOURNAL_FILE_ENV => Some(journal_path.clone()),
        _ => None,
    };

    // Push: empty feed still acks a cursor; the echoed token is masked and
    // the clear secret never appears anywhere in the response tree.
    let pushed = super::sync_transfer::transfer_with(
        &st,
        &json!({ "remote": addr, "token": "averylongtoken123" }),
        "push",
        &get,
    )
    .expect("the push runs");
    assert_eq!(pushed["mode"], "push");
    assert_eq!(pushed["remote"], addr);
    assert_eq!(pushed["encryption"], Value::Bool(true));
    let token = pushed["token"].as_str().expect("masked token");
    assert!(token.starts_with("***"), "{token}");
    assert!(!pushed.to_string().contains("averylongtoken123"));
    let rows = pushed["banks"].as_array().expect("bank rows");
    assert_eq!(rows[0]["bank_id"], "bank-mcp");
    assert!(rows[0]["push_cursor"].as_str().is_some());

    // Pull: short token takes the full-mask branch.
    let pulled = super::sync_transfer::transfer_with(
        &st,
        &json!({ "remote": addr, "mode": "pull", "token": "short" }),
        "pull",
        &get,
    )
    .expect("the pull runs");
    assert_eq!(pulled["token"], "***");
    assert!(pulled["banks"][0]["pull_cursor"].as_str().is_some());

    // Bidirectional: pull then push in one action.
    let bidi = super::sync_transfer::transfer_with(
        &st,
        &json!({ "remote": addr, "mode": "bidirectional" }),
        "pull",
        &get,
    )
    .expect("the bidirectional transfer runs");
    assert_eq!(bidi["mode"], "bidirectional");
    // Bidirectional returns one row per bank per phase: pull rows then push rows.
    assert_eq!(bidi["banks"].as_array().expect("rows").len(), 2);
}

#[test]
fn sync_serve_bind_failure_surfaces_as_error() {
    let st = state();
    let result = dispatch(
        &st,
        "sync_serve",
        &json!({ "dev": true, "bind": "256.0.0.1:1" }),
    );
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
}

#[test]
fn sync_status_reports_the_endpoint_and_auth_it_is_given_without_echoing_the_token() {
    let st = state();
    let out = dispatch(
        &st,
        "sync_status",
        &json!({ "remote": "host:7000", "token": "secret-token-1234" }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["remote"], "host:7000");
    assert_eq!(out["auth"], "token");
    assert!(!out.to_string().contains("secret-token-1234"), "{out}");
}

#[test]
fn sync_status_returns_masked_report_shape() {
    let st = state();
    let result = dispatch(&st, "sync_status", &json!({}));
    assert_eq!(result["ok"], Value::Bool(true), "{result}");
    let device_id = result["device_id"].as_str().expect("device id");
    assert!(!device_id.is_empty(), "{device_id:?}");
    let banks = result["banks"].as_array().expect("banks array");
    assert_eq!(banks.len(), 1);
    assert_eq!(banks[0]["bank_id"], "bank-mcp");
    assert!(banks[0]["push_cursor"].is_null() || banks[0]["push_cursor"].is_string());
    assert!(banks[0]["pending_push"].is_i64());
    // Masked report shape: auth/encryption state, no secret-shaped fields.
    assert_eq!(result["auth"], "none");
    assert_eq!(result["encryption"], Value::Bool(false));
    assert!(result.get("token").is_none());
    assert!(result.get("dek").is_none() && result.get("sync_key").is_none());
    // Read-side routing via dispatch_read as well.
    let direct = crate::additive_tools::dispatch_read(&st, "sync_status", &json!({}))
        .expect("sync_status is a read additive tool");
    assert_eq!(direct.expect("status ok")["device_id"], result["device_id"]);
}

#[test]
fn sync_push_without_remote_fails_as_invalid_argument() {
    let st = state();
    // A keyed node, because the transfer refuses before reading the endpoint
    // otherwise: the endpoint argument is what this test is about.
    let dir = TempDir::new("mcp-push-no-remote");
    let path = dir.key().display().to_string();
    let get = |name: &str| (name == clio_config::SYNC_KEY_FILE_ENV).then(|| path.clone());
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("generate");
    let err = super::sync_transfer::transfer_with(&st, &json!({}), "push", &get)
        .expect_err("a push with no remote is refused");
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument, "{err:?}");
    super::sync_transfer::transfer_with(&st, &json!({}), "pull", &get)
        .expect_err("a pull with no remote is refused");
}

#[test]
fn sync_serve_refuses_anonymous_bind_outside_dev_mode() {
    let st = state();
    let result = dispatch(&st, "sync_serve", &json!({}));
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert_eq!(result["code"], "forbidden");
    // Explicit dev mode satisfies the gate.
    let dev = dispatch(&st, "sync_serve", &json!({ "dev": true, "ttl_secs": 1 }));
    assert_eq!(dev["ok"], Value::Bool(true), "{dev}");
    let addr = dev["addr"].as_str().expect("addr");
    let port: u16 = addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("port");
    assert!(port > 0, "ephemeral port must be resolved: {addr}");
    assert_eq!(dev["ttl_secs"], 1);
    assert_eq!(dev["banks"], json!(["bank-mcp"]));
}

/// The single-token server input was removed: a caller that still supplies
/// `token` is refused before any server work, with the current routes named.
#[test]
fn sync_serve_rejects_the_removed_token_argument() {
    let st = state();
    let result = dispatch(
        &st,
        "sync_serve",
        &json!({ "token": "t0ken-secret-value", "ttl_secs": 1 }),
    );
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert_eq!(result["code"], "invalid_argument", "{result}");
    let message = result["message"].as_str().expect("message");
    assert!(message.contains("token_file"), "{message}");
    assert!(message.contains("clio auth token add"), "{message}");
    // The refusal is the tool call's result and never echoes the secret.
    assert!(
        !result.to_string().contains("t0ken-secret-value"),
        "{result}"
    );
}

/// The serve auth gate is `token_file.is_none() && !dev`: a store file alone
/// satisfies it with no `token`, and an unusable store is refused by the
/// loader rather than quietly enabling an anonymous bind.
#[test]
fn sync_serve_gate_is_token_file_or_explicit_dev_mode() {
    let st = state();
    let store = ScratchStore::new("serve-gate", "s3cret", &["bank-mcp"]);
    let served = dispatch(
        &st,
        "sync_serve",
        &json!({ "token_file": store.path(), "ttl_secs": 1 }),
    );
    assert_eq!(served["ok"], Value::Bool(true), "{served}");
    assert_eq!(served["dev"], Value::Bool(false), "{served}");

    // A store file that does not exist passes the gate and fails at load: the
    // refusal names the store, so the gate never fell back to anonymous.
    let absent = ScratchStore::new("serve-absent", "unused", &["bank-mcp"]);
    std::fs::remove_file(absent.path()).expect("the scratch store is removed");
    let missing = dispatch(
        &st,
        "sync_serve",
        &json!({ "token_file": absent.path(), "ttl_secs": 1 }),
    );
    assert_eq!(missing["ok"], Value::Bool(false), "{missing}");
    assert_eq!(missing["code"], "config_corrupt", "{missing}");
}

#[test]
fn sync_tools_are_schema_published() {
    let defs = crate::schema::catalog_defs();
    for name in [
        "sync_push",
        "sync_pull",
        "sync_status",
        "sync_key_status",
        "sync_serve",
        "sync_ack_skip",
    ] {
        let def = defs
            .iter()
            .find(|d| d.name == name)
            .expect("schema def present");
        assert!(!def.description.is_empty(), "{name}: empty description");
        let tool = def.to_tool();
        let defects = crate::schema::validate_tool_schema(&tool);
        assert!(defects.is_empty(), "{name}: {defects:?}");
    }
    assert!(crate::bound_write_tools().contains(&"sync_push"));
    assert!(crate::bound_write_tools().contains(&"sync_pull"));
    assert!(crate::bound_write_tools().contains(&"sync_serve"));
    assert!(crate::bound_write_tools().contains(&"sync_ack_skip"));
    assert!(crate::bound_read_tools().contains(&"sync_status"));
    assert!(crate::bound_read_tools().contains(&"sync_key_status"));
    assert!(crate::bound_tools().contains(&"sync_status"));
    assert!(crate::bound_tools().contains(&"sync_key_status"));
    // The two printing verbs are CLI-only because they print the sealing key, so
    // no bound list may name them.
    assert!(!crate::bound_tools().contains(&"sync_keygen"));
    assert!(!crate::bound_tools().contains(&"sync_key_export"));
}

/// The transfer tools take no `sync_key` any more. A real dispatch that still
/// supplies one must be rejected before any transfer work, for a string and for
/// an array. `127.0.0.1:1` has no listener, so a run that fell through the guard
/// would surface a transport error instead of `invalid_argument`.
#[test]
fn sync_transfer_rejects_a_supplied_sync_key_string() {
    let st = state();
    for name in ["sync_push", "sync_pull"] {
        let result = dispatch(
            &st,
            name,
            &json!({ "remote": "127.0.0.1:1", "sync_key": "k1" }),
        );
        assert_eq!(result["ok"], Value::Bool(false), "{name}: {result}");
        assert_eq!(result["code"], "invalid_argument", "{name}: {result}");
        let message = result["message"].as_str().expect("message");
        assert!(message.contains("sync key store"), "{name}: {message}");
        assert!(message.contains("clio sync keygen"), "{name}: {message}");
    }
}

#[test]
fn sync_transfer_rejects_a_supplied_sync_key_array() {
    let st = state();
    for name in ["sync_push", "sync_pull"] {
        let result = dispatch(
            &st,
            name,
            &json!({ "remote": "127.0.0.1:1", "sync_key": ["k2", "k1"] }),
        );
        assert_eq!(result["ok"], Value::Bool(false), "{name}: {result}");
        assert_eq!(result["code"], "invalid_argument", "{name}: {result}");
        let message = result["message"].as_str().expect("message");
        assert!(message.contains("sync key store"), "{name}: {message}");
    }
}

#[test]
fn sync_transfer_key_gate_refuses_a_keyless_node() {
    let err = crate::sync_tools::required_custody_cipher(&|_name: &str| None)
        .expect_err("a node with no sync key must not transfer");
    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt, "{err:?}");
    assert!(err.message.contains("clio sync keygen"), "{err}");

    let dir = TempDir::new("mcp-required-cipher");
    let path = dir.key().display().to_string();
    let get = |name: &str| (name == clio_config::SYNC_KEY_FILE_ENV).then(|| path.clone());
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("generate");
    let cipher = crate::sync_tools::required_custody_cipher(&get).expect("a held key answers");
    assert_eq!(cipher.len(), 1, "the key the gate resolved");
}

#[test]
fn sync_unknown_tool_fails_as_not_implemented() {
    let st = state();
    let err = crate::sync_tools::dispatch_inner(&st, "sync_unknown", &json!({}));
    assert_eq!(err.unwrap_err().code, clio_types::ErrorCode::NotImplemented);
    // Full transport dispatch keeps the same failure class.
    let result = dispatch(&st, "sync_unknown", &json!({}));
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert_eq!(result["code"], "not_implemented");
    // Read-side: sync_status in dispatch_read; unknown read names stay None.
    assert!(crate::additive_tools::dispatch_read(&st, "sync_unknown_read", &json!({})).is_none());
}

#[test]
fn sync_banks_argument_is_normalized_fail_closed() {
    let st = state();
    // Non-array banks are rejected.
    let result = dispatch(&st, "sync_status", &json!({ "banks": "bank-mcp" }));
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    let result = dispatch(&st, "sync_status", &json!({ "banks": [7] }));
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    // Empty array resolves to the connection default.
    let result = dispatch(&st, "sync_status", &json!({ "banks": [] }));
    assert_eq!(result["ok"], Value::Bool(true), "{result}");
    assert_eq!(result["banks"][0]["bank_id"], "bank-mcp");
    // Explicit bank selection flows through to the per-bank rows.
    let result = dispatch(&st, "sync_status", &json!({ "banks": ["bank-other"] }));
    assert_eq!(result["banks"][0]["bank_id"], "bank-other");
}
