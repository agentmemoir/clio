// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests `failure list` end to end: its exact and similar modes, its truncation and text view, the `TASK_ID` name its usage hint carries, and the structured fields it returns unchanged from the `failures_for_task` tool.
//!
//! ## Owns
//! - The `failure list` assertions over the in-process dispatcher and over the registry record
//!
//! ## Does not own
//! - The command implementation under test (`cli_read_history`)
//! - The other history and task read commands (`cli_read_history_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on `failure list`.

use super::render_failure_list;
use crate::cli_read_test_support::{call, json_of, run_state, seed_failure, seed_task, state};
use clio_mcp::runtime::McpState;
use serde_json::json;
use std::process::ExitCode;
use std::sync::Arc;

fn history_state() -> Arc<McpState> {
    let s = state();
    seed_task(&s, "bank-a", "t1", "Run tests");
    seed_task(&s, "bank-a", "t1", "Run tests again");
    seed_task(&s, "bank-a", "t1", "Run tests third");
    seed_failure(&s, "bank-a", "t1", 1, "retry once");
    seed_failure(&s, "bank-a", "t1", 2, "retry twice");
    s
}

#[test]
fn failure_list_exact_truncation_and_none() {
    let s = history_state();
    let (code, out, _) = run_state("failure list", &["t1", "--bank", "bank-a"], &s, false);
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["mode"], json!("exact"));
    assert_eq!(value["failures"].as_array().map(Vec::len), Some(2));
    assert_eq!(value["truncated"], json!(false));

    let (_, out, _) = run_state(
        "failure list",
        &["t1", "--bank", "bank-a", "--limit", "1"],
        &s,
        false,
    );
    let page = json_of(&out);
    assert_eq!(page["failures"].as_array().map(Vec::len), Some(1));
    assert_eq!(page["truncated"], json!(true));

    // An argument that names no task and matches no failure is not found.
    let (code, _, err) = run_state(
        "failure list",
        &["zzz-unknown", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("\"code\":\"not_found\""), "{err}");

    let (_, out, _) = run_state("failure list", &["t1", "--bank", "bank-a"], &s, true);
    assert!(out.contains("mode: exact"), "{out}");
    assert!(out.contains("failures: 2 of"), "{out}");

    assert_eq!(
        render_failure_list(&json!({"failures": [{"task_id": "t", "attempt_n": 1}]})),
        "mode: unknown\nfailures: 1 of 0\nt attempt 1"
    );
}

/// `failure list` names its argument `TASK_ID` like the registry record, and
/// the free-text fallback still matches task definitions only: text similar to
/// a definition reaches that task's failures, and a known task id stays exact.
#[test]
fn failure_list_usage_names_the_task_id_and_the_fallback_matches_definitions() {
    let s = history_state();
    let (code, _, err) = run_state("failure list", &[], &s, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("usage: clio failure list TASK_ID"), "{err}");
    assert!(!err.contains("TASK_ID_OR_QUERY"), "{err}");
    let record = crate::cli_command_registry::find("failure list").expect("a record");
    assert_eq!(
        crate::cli_command_registry_test_support::positionals(record),
        [("TASK_ID", true)],
        "the usage hint must name the label the registry record carries"
    );

    // The similarity fallback works over task definitions, so text equal to a
    // definition reaches that task's failures without naming its id.
    let (code, out, err) = run_state(
        "failure list",
        &["Run tests third", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0), "{err}");
    let value = json_of(&out);
    assert_eq!(value["mode"], json!("similar"));
    assert_eq!(value["failures"].as_array().map(Vec::len), Some(2));
}

#[test]
fn failure_list_preserves_mcp_structured_fields() {
    // `lesson_tokens` is a non-secret integer whose key contains the `token`
    // marker. The defensive mask must not rewrite it: the CLI view has to
    // return the same structured result as the MCP tool.
    let s = state();
    seed_failure(&s, "bank-a", "t-parity", 1, "retry once");
    let tool = call(
        &s,
        "failures_for_task",
        json!({"bank": "bank-a", "task_id_or_query": "t-parity"}),
    );
    let (code, out, err) = run_state("failure list", &["t-parity", "--bank", "bank-a"], &s, false);
    assert_eq!(code, ExitCode::from(0), "{err}");
    let cli = json_of(&out);
    assert_eq!(cli["failures"], tool["failures"]);
    let tokens = &cli["failures"][0]["lesson_tokens"];
    assert!(
        tokens.is_number(),
        "lesson_tokens must stay numeric, got {tokens}"
    );
}
