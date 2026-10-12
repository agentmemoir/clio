// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the `canonical put` `context` field:
//! the CLI and the equivalent in-process MCP call must persist it identically,
//! and the CLI must reject an empty or oversized context.
//!
//! ## Owns
//! - CLI/MCP context parity tests for `canonical put`
//! - Context limit tests for `canonical put`
//!
//! ## Does not own
//! - The commands under test (`cli_write_workspace`)
//! - The other write behaviors (see `cli_write_workspace_tests`)
//!
//! ## Boundary
//!
//! Changes to how `canonical put` carries or validates `context` should
//! normally require changes here. Changes to other write behaviors should not
//! require changes here.
//!
//! Keep this module focused on the `canonical put` context field.

use crate::cli_read_test_support::{call, json_of, state};
use crate::cli_write_test_support::run_write_state;
use serde_json::json;
use std::process::ExitCode;
use std::sync::Arc;

/// The stored `context` of one `demo` item, for the CLI/MCP parity asserts.
fn stored_context(s: &Arc<clio_mcp::runtime::McpState>, id: &str) -> Option<String> {
    let read = s.store.get_memory_item("demo", id).unwrap();
    read.expect("item").context
}

/// Required `canonical put` flags shared by the context tests.
fn canonical_put_args(key: &'static str, value: &'static str) -> Vec<&'static str> {
    let mut args = vec![key];
    args.push(value);
    args.extend(["--category", "tool_config", "--epistemic-kind"]);
    args.push("fact");
    args
}

/// CLI `canonical put` and the equivalent in-process MCP `canonical_put` call
/// persist the same source `context`; the no-context form omits the key on both.
#[test]
fn canonical_put_stores_context_identically_to_mcp() {
    let s = state();
    let mut with = canonical_put_args("primary_database_host", "db-a");
    with.push("--context");
    with.push("release week");
    with.push("--bank");
    with.push("demo");
    let (code, out, err) = run_write_state("canonical put", &with, &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let cli_id = json_of(&out)["id"].as_str().expect("stored id").to_owned();
    let mcp_args = json!({
        "key": "secondary_database_host", "value": "db-b", "category": "tool_config",
        "epistemic_kind": "fact", "context": "release week", "bank": "demo"
    });
    let mcp = call(&s, "canonical_put", mcp_args);
    assert_eq!(mcp["ok"], json!(true), "{mcp}");
    let mcp_id = mcp["id"].as_str().expect("stored id");
    let cli_ctx = stored_context(&s, &cli_id);
    assert_eq!(cli_ctx.as_deref(), Some("release week"));
    assert_eq!(cli_ctx, stored_context(&s, mcp_id));
    let mut bare = canonical_put_args("canonical_key", "stored value");
    bare.push("--bank");
    bare.push("demo");
    let (code, out, err) = run_write_state("canonical put", &bare, &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let bare_id = json_of(&out)["id"].as_str().expect("stored id").to_owned();
    let bare_ctx = stored_context(&s, &bare_id);
    assert_eq!(bare_ctx, None);
    let mcp_bare_args = json!({
        "key": "other_key", "value": "stored value mcp", "category": "tool_config",
        "epistemic_kind": "fact", "bank": "demo"
    });
    let mcp_bare = call(&s, "canonical_put", mcp_bare_args);
    assert_eq!(mcp_bare["ok"], json!(true), "{mcp_bare}");
    let mcp_bare_id = mcp_bare["id"].as_str().expect("stored id");
    assert_eq!(bare_ctx, stored_context(&s, mcp_bare_id));
}

#[test]
fn canonical_put_rejects_empty_and_oversized_context() {
    let s = state();
    let oversized = "x".repeat(clio_types::CONTEXT_MAX_BYTES + 1);
    let mut args: Vec<&str> = canonical_put_args("canonical_key", "stored value");
    args.push("--context");
    args.push("");
    let (code, _out, err) = run_write_state("canonical put", &args, &s, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("must not be empty"), "{err}");
    args.pop();
    args.push(oversized.as_str());
    let (code, _out, err) = run_write_state("canonical put", &args, &s, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("at most 4096"), "{err}");
}
