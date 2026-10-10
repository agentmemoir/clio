// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests MCP/CLI context parity end to end: the CLI `remember`
//! binding and the equivalent in-process MCP `store` call produce the same
//! stored item contract for a supplied source `context`.
//!
//! ## Owns
//! - The cross-binding parity scenario for `context`
//!
//! ## Does not own
//! - CLI flag decoding (`cli_write_core`) or MCP decoding (`clio-mcp`)
//! - The shared state helpers (`cli_read_support_tests` /
//!   `cli_write_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{TS, call, json_of, state};
use crate::cli_write_test_support::run_write_state;
use serde_json::json;
use std::process::ExitCode;

#[test]
fn cli_and_mcp_store_the_same_context() {
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "deploys happen on Fridays",
            "--category",
            "task_spec",
            "--context",
            "team chat",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let cli_payload = json_of(&out);
    assert_eq!(cli_payload["ok"], json!(true), "{cli_payload}");
    let cli_id = cli_payload["id"].as_str().expect("stored id").to_owned();

    let mcp_payload = call(
        &s,
        "store",
        json!({
            "item": {"id": "itm-mcp-parity", "subject_id": "s", "gist": "deploys happen on Fridays"},
            "allow_duplicate": true,
            "category": "task_spec",
            "epistemic_kind": "fact",
            "context": "team chat",
            "bank": "demo",
            "actor": "agent",
            "timestamp": TS
        }),
    );
    assert_eq!(mcp_payload["ok"], json!(true), "{mcp_payload}");

    let cli_item = s
        .store
        .get_memory_item("demo", &cli_id)
        .unwrap()
        .expect("cli item");
    let mcp_item = s
        .store
        .get_memory_item("demo", "itm-mcp-parity")
        .unwrap()
        .expect("mcp item");
    assert_eq!(cli_item.context.as_deref(), Some("team chat"));
    assert_eq!(cli_item.context, mcp_item.context);
    assert_eq!(mcp_item.gist, cli_item.gist);
}

/// The CLI `task upsert` binding and the equivalent in-process MCP `task_upsert`
/// call persist the same optional source `context` on the task record (and the
/// no-context form omits the key on both bindings).
#[test]
fn cli_and_mcp_task_upsert_store_the_same_context() {
    let s = state();
    let (code, out, err) = run_write_state(
        "task upsert",
        &[
            "cli-task-1",
            "--definition",
            "Ship the release",
            "--status",
            "open",
            "--context",
            "release week",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let cli_payload = json_of(&out);
    assert_eq!(cli_payload["ok"], json!(true), "{cli_payload}");

    let mcp_payload = call(
        &s,
        "task_upsert",
        json!({
            "task_id": "mcp-task-1",
            "definition": "Ship the release",
            "status": "open",
            "context": "release week",
            "bank": "demo",
            "actor": "agent",
            "timestamp": TS
        }),
    );
    assert_eq!(mcp_payload["ok"], json!(true), "{mcp_payload}");

    let cli_record = clio_history::task_get(&clio_history::TaskGetArgs {
        history: s.history.as_ref(),
        store: s.store.as_ref(),
        bank_id: "demo",
        task_id: "cli-task-1",
    })
    .expect("cli task read")
    .record
    .expect("task stored");
    let mcp_record = clio_history::task_get(&clio_history::TaskGetArgs {
        history: s.history.as_ref(),
        store: s.store.as_ref(),
        bank_id: "demo",
        task_id: "mcp-task-1",
    })
    .expect("mcp task read")
    .record
    .expect("task stored");
    assert_eq!(cli_record.context.as_deref(), Some("release week"));
    assert_eq!(cli_record.context, mcp_record.context);
}
