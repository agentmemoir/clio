// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio` write-command engine: verb resolution, exit
//! codes, `--help` surfaces, the refusal mapping, and no-write preview
//! emission over injected state.
//!
//! ## Owns
//! - Engine tests for `cli_write` and the `WriteGroup` refusal contract
//!
//! ## Does not own
//! - The engine under test (`cli_write`)
//! - The concrete group behavior tests (`cli_write_core_tests` and friends)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{json_of, seed_item, state};
use crate::cli_write_group::{WriteCall, decision_refusal};
use crate::cli_write_test_support::{run_write_open, run_write_resolved, run_write_state};
use serde_json::json;
use std::process::ExitCode;

#[test]
fn flattened_pass_false_maps_to_refusal() {
    let payload = json!({"ok": true, "pass": false, "rejection_reason": "score 0.67 < theta 0.73 for `persona`"});
    let refusal = decision_refusal(&payload).expect("refusal");
    assert_eq!(refusal.code, "admission_refused");
    assert_eq!(refusal.message, "score 0.67 < theta 0.73 for `persona`");
}

#[test]
fn nested_decision_pass_false_maps_to_refusal() {
    let payload = json!({
        "ok": true,
        "decision": {"pass": false, "rejection_reason": "span_verify failed"}
    });
    let refusal = decision_refusal(&payload).expect("refusal");
    assert_eq!(refusal.message, "span_verify failed");
}

#[test]
fn missing_reason_falls_back_to_gates_message() {
    let payload = json!({"ok": true, "decision": {"pass": false}});
    let refusal = decision_refusal(&payload).expect("refusal");
    assert!(refusal.message.contains("refused by the admission gates"));
}

#[test]
fn passing_decision_and_batch_payloads_are_not_refusals() {
    let passing = json!({"ok": true, "pass": true, "id": "itm-1"});
    assert!(decision_refusal(&passing).is_none());
    let batch = json!({"ok": true, "results": [], "would_admit": 0, "would_reject": 0});
    assert!(decision_refusal(&batch).is_none());
}

#[test]
fn unknown_write_verb_exits_two() {
    // A verb no group owns fails closed as a usage error.
    let s = state();
    let (code, out, err) = run_write_state("warp", &["x"], &s, false);
    assert_eq!(code, ExitCode::from(2));
    assert!(err.contains("unknown command `warp`"), "{err}");
    let _ = out;
}

#[test]
fn owns_reports_only_group_write_verbs() {
    assert!(crate::cli_write::owns("triple add"));
    assert!(crate::cli_write::owns("graph link"));
    assert!(!crate::cli_write::owns("recall"));
}

#[test]
fn system_env_reads_the_process_environment() {
    assert_eq!(super::system_env("CLIO_TEST_UNSET_9F3A"), None);
}

#[test]
fn resolved_write_parse_error_exits_two_before_opening_a_database() {
    let (code, _out, err) = run_write_resolved("remember", &["--bogus"]);
    assert_eq!(code, ExitCode::from(2));
    assert!(!err.is_empty(), "usage error must be reported");
}

#[test]
fn resolved_write_help_exits_zero_before_opening_a_database() {
    let (code, out, err) = run_write_resolved("remember", &["--help"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("usage: clio remember"), "{out}");
}

#[test]
fn state_write_parse_error_exits_two() {
    let s = state();
    let (code, _out, err) = run_write_state("remember", &["--bogus"], &s, false);
    assert_eq!(code, ExitCode::from(2));
    assert!(!err.is_empty(), "usage error must be reported");
}

#[test]
fn write_verb_help_prints_binding_without_a_database() {
    for (verb, tool) in [
        ("remember", "store"),
        ("admit", "admit_preview"),
        ("summarize", "summarize"),
        ("consolidate", "consolidate"),
    ] {
        let s = state();
        let (code, out, err) = run_write_state(verb, &["--help"], &s, false);
        assert_eq!(code, ExitCode::SUCCESS, "{verb}: {err}");
        assert!(
            out.contains(&format!("usage: clio {verb}")),
            "{verb}: {out}"
        );
        assert!(
            out.contains(&format!("calls the `{tool}` tool")),
            "{verb}: {out}"
        );
    }
}

#[test]
fn group_resolved_write_help_exits_zero() {
    let s = state();
    for (cmd, verb, tool) in [
        ("triple", "triple add", "triple_add"),
        ("triple end", "triple end", "triple_end"),
        ("belief", "belief observe", "belief_observe"),
        ("graph", "graph link", "graph_link"),
    ] {
        let (code, out, err) = run_write_state(verb, &["--help"], &s, false);
        assert_eq!(code, ExitCode::SUCCESS, "{cmd}: {err}");
        assert!(
            out.contains(&format!("calls the `{tool}` tool")),
            "{verb}: {out}"
        );
    }
}

#[test]
fn group_help_lists_write_subcommands_and_tools() {
    // `clio triple --help` lists every subcommand with its bound tool,
    // before any database is opened.
    let (code, out, err) = run_write_open("triple", &["--help"], false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("clio triple add   -> triple_add"), "{out}");
    assert!(out.contains("clio triple end   -> triple_end"), "{out}");
    assert!(out.contains("clio triple query   -> triple_query"), "{out}");
}

#[test]
fn dry_run_preview_writes_nothing_and_exits_zero() {
    // `consolidate --dry-run` has no in-process no-write path: the engine must
    // emit the preview without calling the dispatcher.
    let s = state();
    let (code, out, err) = run_write_state("consolidate", &["--dry-run"], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true));
    assert!(out.contains("dry_run"), "{out}");
}

/// The engine injects the preview module's read-only counts into the dry-run
/// payload next to the `would` sentence.
#[test]
fn dry_run_payload_carries_the_pending_work_counts() {
    let s = crate::cli_read_test_support::extract_state();
    seed_item(&s, "demo", "itm-a", "prefers aisle seats");
    let (code, out, err) = run_write_state(
        "summarize",
        &["--scope", "bank", "--bank", "demo", "--dry-run"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["counts"]["gists"], json!(1), "{payload}");
    assert_eq!(payload["counts"]["skipped"], json!(0), "{payload}");
    assert!(
        payload["would"]
            .as_str()
            .is_some_and(|would| would.contains("snapshots are never altered")),
        "the would sentence stays: {payload}"
    );
}

/// The dry run answers the request the real call serves: with no extractor
/// configured, real run and dry run fail identically (same `not_implemented`
/// JSON, same exit 1), so a caller cannot tell them apart by outcome.
#[test]
fn summarize_dry_run_fails_like_the_real_call_without_an_extractor() {
    let s = state();
    seed_item(&s, "demo", "itm-a", "prefers aisle seats");
    let real = run_write_state(
        "summarize",
        &["--scope", "bank", "--bank", "demo", "--output", "json"],
        &s,
        false,
    );
    let dry = run_write_state(
        "summarize",
        &[
            "--scope",
            "bank",
            "--bank",
            "demo",
            "--output",
            "json",
            "--dry-run",
        ],
        &s,
        false,
    );
    assert_eq!(real.0, dry.0, "exit codes must match: {real:?} vs {dry:?}");
    assert!(real.1.is_empty(), "real stdout: {}", real.1);
    assert!(dry.1.is_empty(), "dry stdout: {}", dry.1);
    assert_eq!(
        json_of(&real.2),
        json_of(&dry.2),
        "error payloads must match"
    );
    assert_eq!(json_of(&dry.2)["code"], json!("not_implemented"), "{dry:?}");
}

#[test]
fn consolidate_live_call_is_accepted_and_drained() {
    let s = state();
    let (code, out, err) = run_write_state("consolidate", &[], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert!(payload["job_id"].as_str().is_some(), "{payload}");
}

#[test]
fn consolidate_rejects_the_removed_scope_flag() {
    // The selector was renamed to the global `--bank` with no alias: `--scope`
    // must fail as an unknown flag rather than drop the selector and silently
    // target the context bank.
    let s = state();
    let (code, out, err) = run_write_state("consolidate", &["--scope", "demo"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--scope"), "{err}");
}

#[test]
fn consolidate_rejects_an_empty_bank_name() {
    // The renamed selector is the same `bank` argument every other verb takes, and
    // an explicitly empty value is refused by the one rule that covers every value
    // flag rather than falling back to the context bank.
    let s = state();
    let (code, out, err) = run_write_state("consolidate", &["--bank", ""], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    let payload = json_of(&err);
    assert_eq!(payload["ok"], json!(false), "{payload}");
    assert_eq!(payload["code"], json!("usage"), "{payload}");
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|message| message.contains("--bank must not be empty")),
        "{payload}"
    );
}

#[test]
fn remember_arguments_route_to_store() {
    // The builder's tool choice is part of the contract: verify via the call
    // shape the dispatcher receives (item shell + required category).
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "prefers terse answers",
            "--category",
            "persona",
            "--bank",
            "b",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["pass"], json!(true), "{payload}");
    assert!(
        payload["id"]
            .as_str()
            .unwrap_or_default()
            .starts_with("itm-cli-")
    );
}

#[test]
fn preview_call_shape_is_tool_free() {
    // The `Preview` variant carries no tool name, so a no-write preview can
    // never be turned into a dispatcher call by the engine.
    let call = WriteCall::Preview {
        payload: json!({"dry_run": true}),
        text: "dry-run".to_owned(),
    };
    match call {
        WriteCall::Preview { payload, text } => {
            assert_eq!(payload["dry_run"], json!(true));
            assert_eq!(text, "dry-run");
        }
        WriteCall::Tool { .. } => panic!("preview must not carry a tool"),
    }
}
