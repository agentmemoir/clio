// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the workspace `clio` write commands: the scratchpad trio
//! (write/read/clear, ephemeral only) and `canonical put|get`, including the
//! Long-term isolation guarantee and dry-run zero-write behavior.
//!
//! ## Owns
//! - Behavior and golden tests for `cli_write_workspace`
//!
//! ## Does not own
//! - The commands under test (`cli_write_workspace`)
//! - The engine and shared fixtures (see `cli_write` / `cli_write_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{call, json_of, state};
use crate::cli_write_group::WriteGroup;
use crate::cli_write_test_support::{run_write_resolved, run_write_state};
use serde_json::json;
use std::process::ExitCode;
use std::sync::Arc;

/// Long-term item count for one bank (the isolation baseline).
fn active_items(state: &Arc<clio_mcp::runtime::McpState>) -> u64 {
    let report = call(state, "stats", json!({"bank": "demo"}));
    report["items_active"].as_u64().unwrap_or_default()
}

#[test]
fn scratchpad_write_then_read_round_trip_in_one_state() {
    // The scratchpad is per-process memory by design; the round trip works
    // because the injected state is shared across the two calls.
    let s = state();
    let (code, out, err) = run_write_state(
        "scratchpad write",
        &["flight-seat", "12A", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["stored"], json!(true), "{payload}");
    assert_eq!(payload["bytes"], json!(3), "{payload}");
    assert_eq!(payload["ephemeral"], json!(true), "{payload}");
    let (code, out, err) = run_write_state(
        "scratchpad read",
        &["flight-seat", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(out.trim(), "12A", "{out}");
}

#[test]
fn scratchpad_session_flag_scopes_the_pad() {
    let s = state();
    let (code, _, err) = run_write_state(
        "scratchpad write",
        &["note", "ephemeral", "--session", "ops", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    // The same session reads it back; the default session does not see it.
    let (code, out, err) = run_write_state(
        "scratchpad read",
        &["note", "--session", "ops", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(json_of(&out)["content"], json!("ephemeral"), "{out}");
    let (code, _, err) = run_write_state("scratchpad read", &["note", "--bank", "demo"], &s, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
}

#[test]
fn scratchpad_write_leaves_long_term_counts_untouched() {
    // The scratchpad is in-process memory only; it must never create or
    // alter a durable long-term item row.
    let s = state();
    let before = active_items(&s);
    let (code, out, err) = run_write_state(
        "scratchpad write",
        &["note", "ephemeral", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(json_of(&out)["ephemeral"], json!(true), "{out}");
    assert_eq!(active_items(&s), before, "scratchpad must never index");
}

#[test]
fn scratchpad_clear_key_then_read_exits_one() {
    let s = state();
    let (code, _, err) = run_write_state(
        "scratchpad write",
        &["note", "ephemeral", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let (code, _, err) =
        run_write_state("scratchpad clear", &["note", "--bank", "demo"], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let (code, _, err) = run_write_state("scratchpad read", &["note", "--bank", "demo"], &s, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("not found"), "{err}");
}

#[test]
fn scratchpad_clear_without_key_purges_the_session_pad() {
    let s = state();
    for key in ["a", "b"] {
        let (code, _, err) = run_write_state(
            "scratchpad write",
            &[key, "ephemeral", "--bank", "demo"],
            &s,
            false,
        );
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
    }
    let (code, _, err) = run_write_state("scratchpad clear", &["--bank", "demo"], &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let (code, out, err) = run_write_state("scratchpad read", &["--bank", "demo"], &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("entries: 0 (total_bytes: 0)"), "{out}");
}

#[test]
fn scratchpad_usage_errors_exit_two() {
    let s = state();
    let (code, _, err) = run_write_state(
        "scratchpad write",
        &["only-key", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("KEY and CONTENT"), "{err}");
    let (code, _, err) = run_write_state(
        "scratchpad read",
        &["one", "two", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("unexpected extra"), "{err}");
}

#[test]
fn canonical_put_then_get_round_trip() {
    let s = state();
    let (code, out, err) = run_write_state(
        "canonical put",
        &[
            "primary_database_host",
            "db-a",
            "--category",
            "tool_config",
            "--epistemic-kind",
            "fact",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["pass"], json!(true), "{payload}");
    assert!(payload["id"].as_str().is_some(), "{payload}");
    let (code, out, err) = run_write_state(
        "canonical get",
        &["primary_database_host", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        out.contains("canonical `primary_database_host` = db-a (item_id: "),
        "{out}"
    );
    assert!(out.contains(", valid_from: "), "{out}");
    let (code, out, err) = run_write_state(
        "canonical get",
        &["primary_database_host", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["value"], json!("db-a"), "{payload}");
    assert!(payload["item_id"].as_str().is_some(), "{payload}");
    assert!(payload["valid_from"].as_str().is_some(), "{payload}");
}

#[test]
fn canonical_put_dry_run_previews_without_writing() {
    let s = state();
    let before = active_items(&s);
    let (code, out, err) = run_write_state(
        "canonical put",
        &[
            "primary_database_host",
            "db-a",
            "--category",
            "tool_config",
            "--epistemic-kind",
            "fact",
            "--dry-run",
            "--bank",
            "demo",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        out.contains("dry-run: would put the canonical slot (zero writes)"),
        "{out}"
    );
    assert_eq!(active_items(&s), before, "dry-run must write nothing");
    let (code, out, _) = run_write_state(
        "canonical put",
        &[
            "k",
            "v",
            "--category",
            "tool_config",
            "--epistemic-kind",
            "fact",
            "--dry-run",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(json_of(&out)["dry_run"], json!(true));
}

#[test]
fn canonical_get_of_an_empty_slot_exits_one() {
    let s = state();
    let (code, _, err) = run_write_state("canonical get", &["nope", "--bank", "demo"], &s, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("empty"), "{err}");
}

#[test]
fn canonical_put_requires_two_positionals_and_flags() {
    let s = state();
    let (code, _, err) = run_write_state(
        "canonical put",
        &[
            "only-key",
            "--category",
            "tool_config",
            "--epistemic-kind",
            "fact",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("KEY and VALUE"), "{err}");
    let (code, _, err) = run_write_state(
        "canonical put",
        &["k", "v", "--epistemic-kind", "fact"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--category"), "{err}");
    let (code, _, err) = run_write_state(
        "canonical put",
        &["k", "v", "--category", "tool_config"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--epistemic-kind"), "{err}");
    let (code, _, err) = run_write_state(
        "canonical put",
        &[
            "k",
            "v",
            "--category",
            "tool_config",
            "--epistemic-kind",
            "fact",
            "--confidence",
            "abc",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--confidence"), "{err}");
}

#[test]
fn canonical_put_help_exits_zero() {
    let (code, out, err) = run_write_resolved("canonical put", &["--help"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("canonical put KEY VALUE"), "{out}");
}

#[test]
fn render_views_fall_back_deterministically() {
    // The fallback arms keep text output stable when an optional field is
    // absent from a tool payload.
    assert_eq!(
        super::render_scratchpad_write(&json!({"key": "k", "bytes": 3, "lifetime": "process"})),
        "scratchpad `k` stored for this process only (3 bytes)"
    );
    assert_eq!(
        super::render_scratchpad_read(&json!({"key": "k", "content": "12A"})),
        "12A"
    );
    // Non-string stored content (the tool parses JSON content) renders compact.
    assert_eq!(
        super::render_scratchpad_read(&json!({"key": "k", "content": {"a": 1}})),
        "{\"a\":1}"
    );
    assert_eq!(
        super::render_scratchpad_read(
            &json!({"entries": [{"key": "k", "content": "v"}], "total_bytes": 1})
        ),
        "entries: 1 (total_bytes: 1)"
    );
    assert_eq!(
        super::render_scratchpad_read(&json!({})),
        "entries: 0 (total_bytes: 0)"
    );
    assert_eq!(
        super::render_canonical_get(&json!({
            "key": "k", "value": "v", "item_id": "itm-1", "valid_from": "t"
        })),
        "canonical `k` = v (item_id: itm-1, valid_from: t)"
    );
}

#[test]
fn canonical_put_views_never_claim_writes_for_previews() {
    let preview = json!({"dry_run": true, "pass": false, "rejection_reason": "too weak"});
    assert_eq!(
        super::render_canonical_put(&preview),
        "dry-run: would put the canonical slot (zero writes)"
    );
    let stored = json!({"id": "itm-9", "pass": true});
    assert_eq!(
        super::render_canonical_put(&stored),
        "stored canonical (id itm-9)"
    );
    // A dry-run payload is a read: even a rejecting decision exits 0.
    assert!(
        super::WorkspaceWriteGroup
            .refusal("canonical put", &preview)
            .is_none()
    );
    let refused = json!({"pass": false, "rejection_reason": "taxonomy rejected"});
    let refusal = super::WorkspaceWriteGroup
        .refusal("canonical put", &refused)
        .expect("refusal");
    assert_eq!(refusal.code, "admission_refused");
    assert_eq!(refusal.message, "taxonomy rejected");
}

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
