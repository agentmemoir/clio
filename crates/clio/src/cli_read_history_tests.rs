// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the history and task read commands (`task get`,
//! `task history`, `failure list`, `memtree query`, `memtree get`,
//! `history temporal`) through the in-process dispatcher.
//!
//! ## Owns
//! - Assertions over the history group's builds, decorations, and text views
//!
//! ## Does not own
//! - The command implementation under test (`cli_read_history`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{
    HistoryGroup, render_failure_list, render_node, render_node_list, render_task_get,
    render_task_history, render_temporal,
};
use crate::cli_args::ParsedArgs;
use crate::cli_read_group::ReadGroup;
use crate::cli_read_test_support::{
    call, json_of, run_state, seed_failure, seed_item, seed_task, seed_triple, state,
};
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
    seed_item(&s, "bank-a", "itm-1", "terse answers");
    seed_triple(&s, "bank-a", "rust", "used_by", "agent");
    s
}

#[test]
fn task_get_success_missing_and_text() {
    let s = history_state();
    let (code, out, err) = run_state("task get", &["t1", "--bank", "bank-a"], &s, false);
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["record"]["task_id"], json!("t1"));
    assert!(err.is_empty(), "{err}");

    let (_, out, _) = run_state("task get", &["t1", "--bank", "bank-a"], &s, true);
    assert!(out.contains("task_id: t1"), "{out}");
    assert!(out.contains("version: 3"), "{out}");

    // A task the bank does not hold is not found, not an empty record.
    let (code, out, err) = run_state("task get", &["ghost", "--bank", "bank-a"], &s, false);
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(err.contains("\"code\":\"not_found\""), "{err}");

    assert_eq!(render_task_get(&json!({})), "no task");
    assert_eq!(
        render_task_get(&json!({
            "record": {"task_id": "t", "version": 2, "status": "done"},
            "ancestor_gist": "summary"
        })),
        "task_id: t\nversion: 2\nstatus: done\nancestor_gist: summary"
    );
}

#[test]
fn task_history_limit_and_truncation() {
    let s = history_state();
    let (_, out, _) = run_state("task history", &["t1", "--bank", "bank-a"], &s, false);
    let all = json_of(&out);
    assert_eq!(all["records"].as_array().map(Vec::len), Some(3));
    assert_eq!(all["truncated"], json!(false));

    let (_, out, _) = run_state(
        "task history",
        &["t1", "--bank", "bank-a", "--limit", "2"],
        &s,
        false,
    );
    let page = json_of(&out);
    assert_eq!(page["records"].as_array().map(Vec::len), Some(2));
    assert_eq!(page["limit"], json!(2));
    assert_eq!(page["truncated"], json!(true));

    let (_, out, _) = run_state(
        "task history",
        &["t1", "--bank", "bank-a", "--limit", "2", "--explain"],
        &s,
        true,
    );
    assert!(out.contains("records: 2 of 2"), "{out}");

    assert_eq!(
        render_task_history(&json!({"records": []})),
        "records: 0 of 0"
    );
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

#[test]
fn memtree_query_get_and_truncation() {
    let s = history_state();
    let (code, out, _) = run_state("memtree query", &["--bank", "bank-a"], &s, false);
    assert_eq!(code, ExitCode::from(0));
    let nodes = json_of(&out);
    let list = nodes["nodes"].as_array().expect("nodes");
    assert!(!list.is_empty(), "leaf attach must create a node");
    let node_id = list[0]["id"].as_str().expect("node id").to_owned();

    let (_, out, _) = run_state("memtree get", &[&node_id, "--bank", "bank-a"], &s, false);
    assert_eq!(json_of(&out)["ok"], json!(true));

    let (_, out, _) = run_state(
        "memtree query",
        &[
            "--bank",
            "bank-a",
            "--limit",
            "1",
            "--depth",
            "0",
            "--time-start",
            "2020-01-01",
            "--time-end",
            "2030-01-01",
        ],
        &s,
        false,
    );
    let page = json_of(&out);
    assert_eq!(page["nodes"].as_array().map(Vec::len), Some(1));
    assert_eq!(page["truncated"], json!(true));

    let (_, out, _) = run_state(
        "memtree query",
        &["--bank", "bank-a", "--limit", "50"],
        &s,
        false,
    );
    assert_eq!(json_of(&out)["truncated"], json!(false));

    let (_, out, _) = run_state("memtree query", &["--bank", "bank-a"], &s, true);
    assert!(out.contains("nodes: "), "{out}");
    assert!(out.contains(&node_id), "{out}");

    assert_eq!(render_node_list("nodes", &json!({})), "nodes: 0");
    assert_eq!(
        render_node(&json!({"id": "n1", "depth": 2, "summary_gist": "g"})),
        "id: n1\ndepth: 2\nsummary_gist: g"
    );
    assert_eq!(render_node(&json!({})), "id: ?\ndepth: 0");
}

#[test]
fn temporal_history_triple_and_error() {
    let s = history_state();
    let (code, out, _) = run_state(
        "history temporal",
        &["triple:rust:used_by", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["target_kind"], json!("triple"));
    assert_eq!(value["entries"].as_array().map(Vec::len), Some(1));

    let (_, out, _) = run_state(
        "history temporal",
        &["triple:rust:used_by", "--bank", "bank-a"],
        &s,
        true,
    );
    assert!(out.contains("target_kind: triple"), "{out}");

    let (code, _, _) = run_state(
        "history temporal",
        &["bogus:thing", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(1));

    assert_eq!(
        render_temporal(&json!({})),
        "target_kind: unknown\nentries: 0"
    );
}

#[test]
fn usage_errors_exit_two() {
    let s = history_state();
    let cases: &[(&str, &[&str])] = &[
        ("task get", &[]),
        ("task get", &["a", "b"]),
        ("task history", &[]),
        ("task history", &["t1", "--limit", "x"]),
        ("failure list", &[]),
        ("failure list", &["t1", "--limit", "x"]),
        ("memtree query", &["extra"]),
        ("memtree query", &["--depth", "x"]),
        ("memtree get", &[]),
        ("history temporal", &[]),
        ("history temporal", &["t", "--time-axis", "bogus"]),
        ("task get", &["t1", "--limt", "2"]),
    ];
    for &(verb, args) in cases {
        let (code, out, err) = run_state(verb, args, &s, false);
        assert_eq!(code, ExitCode::from(2), "{verb} {args:?}");
        assert!(out.is_empty(), "{verb} {args:?} stdout: {out}");
        assert!(err.contains("\"code\":\"usage\""), "{verb} {args:?}: {err}");
    }
}

/// Every verb's spec is its record's, and the accepted set that record carries is
/// the one the group's own list held: same flags, same order, globals included.
#[test]
fn spec_answers_from_the_registry_with_the_original_accepted_set() {
    let expected: &[(&str, &[&str], &[&str])] = &[
        (
            "task get",
            &["db", "backend", "bank", "actor", "output"],
            &["no-input", "yes", "explain"],
        ),
        (
            "task history",
            &["db", "backend", "bank", "actor", "output", "limit"],
            &["no-input", "yes", "explain"],
        ),
        (
            "failure list",
            &["db", "backend", "bank", "actor", "output", "limit"],
            &["no-input", "yes", "explain"],
        ),
        (
            "memtree query",
            &[
                "db",
                "backend",
                "bank",
                "actor",
                "output",
                "time-start",
                "time-end",
                "task-id",
                "depth",
                "limit",
            ],
            &["no-input", "yes", "explain"],
        ),
        (
            "memtree get",
            &["db", "backend", "bank", "actor", "output"],
            &["no-input", "yes", "explain"],
        ),
        (
            "history temporal",
            &[
                "db",
                "backend",
                "bank",
                "actor",
                "output",
                "as-of",
                "time-axis",
            ],
            &["no-input", "yes", "explain"],
        ),
    ];
    let group = HistoryGroup;
    assert_eq!(group.verbs().len(), expected.len(), "every verb is covered");
    for &(verb, value, boolean) in expected {
        let spec = group.spec(verb);
        assert_eq!(spec.value_flags(), value, "{verb} value flags");
        assert_eq!(spec.bool_flags(), boolean, "{verb} bool flags");
        let record = crate::cli_command_registry::find(verb).expect("a record");
        assert_eq!(
            spec.value_flags(),
            record.parser_spec().value_flags(),
            "{verb}: the group must answer with the record"
        );
        assert_eq!(
            spec.bool_flags(),
            record.parser_spec().bool_flags(),
            "{verb}: the group must answer with the record"
        );
    }
}

#[test]
fn history_views_mask_planted_secrets() {
    // Task definitions and failure lessons are echoed in the JSON payloads,
    // so a secret planted inside their prose must never leave the command.
    let s = state();
    seed_task(
        &s,
        "bank-a",
        "t-secret",
        "deploy with api_key: sk-live-supersecret",
    );
    seed_failure(
        &s,
        "bank-a",
        "t-secret",
        1,
        "lesson: password=hunter2secret",
    );
    for (verb, args) in [
        ("task get", vec!["t-secret", "--bank", "bank-a"]),
        ("task history", vec!["t-secret", "--bank", "bank-a"]),
        ("failure list", vec!["t-secret", "--bank", "bank-a"]),
    ] {
        let (code, out, err) = run_state(verb, &args, &s, false);
        assert_eq!(code, ExitCode::from(0), "{verb}");
        assert!(err.is_empty(), "{verb}: {err}");
        assert!(!out.contains("supersecret"), "{verb} leaked: {out}");
        assert!(out.contains("****"), "{verb} did not mask: {out}");
    }

    // A synthetic secret-keyed field proves the structural mask pass: any
    // secret-shaped key is masked even where the prose scrub does not apply.
    let mut payload = json!({
        "record": {"definition": "x", "api_key": "sk-live-supersecret"},
    });
    HistoryGroup.decorate("task get", &ParsedArgs::default(), &mut payload);
    let masked = payload.to_string();
    assert!(!masked.contains("supersecret"), "{masked}");
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
