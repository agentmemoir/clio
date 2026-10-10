// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio` read-command surface end to end through the
//! in-process MCP dispatcher: JSON/text rendering, exit codes, usage errors,
//! the `inspect` truncation marker, and credential masking.
//!
//! ## Owns
//! - Assertions over `run_with_state` / `run_verb` / `call_tool` / `render_text`
//!
//! ## Does not own
//! - The read-command implementation under test (`cli_read`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the shared read-contract tests.

use super::*;
use crate::cli_style::Palette;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};

fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn utf8(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("utf8")
}

/// Seed one admitted item and register its lexical doc so retrieval can hit it.
fn seed(state: &McpState, bank: &str, id: &str, gist: &str) {
    let args = json!({
        "item": {
            "id": id,
            "subject_id": "s",
            "gist": gist,
            "snapshot": {"entity": gist, "amount": 1, "date": "2024-01-01"}
        },
        "category": "task_spec",
        "epistemic_kind": "fact",
        "source_text": format!("The user prefers {gist} (1 note since 2024-01-01)."),
        "bank": bank,
        "actor": "agent",
        "timestamp": "2024-01-01T00:00:00Z"
    });
    let stored = clio_mcp::write_tools::dispatch(state, "store", &args);
    assert_eq!(stored["ok"], json!(true), "seed {id}: {stored}");
    state
        .index
        .put_lexical_doc(bank, id, gist)
        .expect("lexical doc");
}

fn state_with_items() -> Arc<McpState> {
    let state = McpState::open(&McpOpenOptions::default()).expect("state");
    seed(&state, "bank-a", "itm-1", "terse answers");
    seed(&state, "bank-a", "itm-2", "verbose replies");
    state
}

fn run_state(
    verb: &str,
    args: &[&str],
    state: &Arc<McpState>,
    is_tty: bool,
) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty,
            no_color: true,
            env: &env,
        };
        run_with_state(verb, tokens(args), state, &mut io)
    };
    (code, utf8(out), utf8(err))
}

fn run_open(verb: &str, args: &[&str]) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty: false,
            no_color: true,
            env: &env,
        };
        run_verb(verb, tokens(args), &mut io)
    };
    (code, utf8(out), utf8(err))
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text.trim()).expect("json output")
}

#[test]
fn recall_passes_domains_as_of_and_time_axis() {
    let state = state_with_items();
    let (code, out, _) = run_state(
        "recall",
        &[
            "terse",
            "--bank",
            "bank-a",
            "--domains",
            "semantic,episodic",
            "--as-of",
            "2024-06-01T00:00:00Z",
            "--time-axis",
            "transaction",
            "--limit",
            "5",
        ],
        &state,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    assert_eq!(json_of(&out)["ok"], json!(true));
}

#[test]
fn get_existing_and_missing() {
    let state = state_with_items();
    let (code, out, err) = run_state("get", &["itm-1", "--bank", "bank-a"], &state, false);
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["id"], json!("itm-1"));
    assert_eq!(value["category"], json!("task_spec"));
    assert!(err.is_empty(), "stderr: {err}");

    let (code, out, err) = run_state("get", &["ghost", "--bank", "bank-a"], &state, false);
    assert_eq!(code, ExitCode::from(1));
    assert!(out.is_empty(), "stdout: {out}");
    let envelope = json_of(&err);
    assert_eq!(envelope["ok"], json!(false));
    assert_eq!(envelope["code"], json!("item_not_found"));
}

#[test]
fn show_snapshot_and_gist_and_conflict() {
    let state = state_with_items();
    let (_, out, _) = run_state(
        "show",
        &["itm-1", "--snapshot", "--bank", "bank-a"],
        &state,
        false,
    );
    let snapshot = json_of(&out);
    assert_eq!(snapshot["authoritative"], json!(true));
    assert_eq!(snapshot["snapshot"]["amount"], json!(1));

    let (_, out, _) = run_state(
        "show",
        &["itm-1", "--gist", "--bank", "bank-a"],
        &state,
        false,
    );
    let gist = json_of(&out);
    assert_eq!(gist["authoritative"], json!(false));
    assert_eq!(gist["gist"], json!("terse answers"));

    let (code, out, err) = run_state(
        "show",
        &["itm-1", "--snapshot", "--gist", "--bank", "bank-a"],
        &state,
        false,
    );
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(json_of(&err)["code"], json!("usage"));
}

#[test]
fn inspect_marks_truncation() {
    let state = state_with_items();
    let (_, out, _) = run_state("inspect", &["--bank", "bank-a"], &state, false);
    let all = json_of(&out);
    assert_eq!(all["ok"], json!(true));
    assert_eq!(all["total"], json!(2));
    assert_eq!(all["truncated"], json!(false));

    let (_, out, _) = run_state(
        "inspect",
        &["--bank", "bank-a", "--limit", "1"],
        &state,
        false,
    );
    let page = json_of(&out);
    assert_eq!(page["truncated"], json!(true));
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1));

    let (_, out, _) = run_state(
        "inspect",
        &["--bank", "bank-a", "--offset", "1", "--limit", "1"],
        &state,
        false,
    );
    assert_eq!(json_of(&out)["truncated"], json!(false));
}

#[test]
fn inspect_filters_and_individual_flags() {
    let state = state_with_items();
    let (_, out, _) = run_state(
        "inspect",
        &[
            "--bank",
            "bank-a",
            "--filter",
            r#"{"category":"task_spec"}"#,
            "--include-discarded",
        ],
        &state,
        false,
    );
    let value = json_of(&out);
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["total"], json!(2));
    assert_eq!(value["include_discarded"], json!(true));

    let (_, out, _) = run_state(
        "inspect",
        &[
            "--bank",
            "bank-a",
            "--category",
            "task_spec",
            "--epistemic-kind",
            "belief",
            "--validity",
            "active",
            "--time-start",
            "2024-01-01T00:00:00Z",
            "--time-end",
            "2030-01-01T00:00:00Z",
        ],
        &state,
        false,
    );
    assert_eq!(json_of(&out)["ok"], json!(true));
}

#[test]
fn stats_json_and_text() {
    let state = state_with_items();
    let (code, out, _) = run_state("stats", &["--bank", "bank-a"], &state, false);
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["bank"], json!("bank-a"));
    assert_eq!(value["items_total"], json!(2));

    let (_, out, _) = run_state("stats", &["--bank", "bank-a"], &state, true);
    assert!(out.contains("bank: bank-a"), "{out}");
    assert!(out.contains("items_total: 2"), "{out}");
    assert!(out.contains("items_active: 2"), "{out}");
    assert!(out.contains("items_discarded: 0"), "{out}");
}

#[test]
fn text_views_for_get_and_inspect() {
    let state = state_with_items();
    let (_, out, _) = run_state("get", &["itm-1", "--bank", "bank-a"], &state, true);
    assert!(out.contains("id: itm-1"), "{out}");
    assert!(out.contains("category: task_spec"), "{out}");
    assert!(out.contains("epistemic_kind: fact"), "{out}");
    assert!(out.contains("gist: terse answers"), "{out}");
    assert!(out.contains("snapshot:"), "{out}");

    let (_, out, _) = run_state(
        "show",
        &["itm-1", "--snapshot", "--bank", "bank-a"],
        &state,
        true,
    );
    assert!(out.contains("id: itm-1"), "{out}");
    assert!(out.contains("authoritative: true"), "{out}");

    let (_, out, _) = run_state("inspect", &["--bank", "bank-a"], &state, true);
    assert!(out.contains("items: 2 of 2"), "{out}");
    assert!(out.contains("itm-1"), "{out}");
    assert!(out.contains("itm-2"), "{out}");

    // The text dispatcher's fallback arm renders the stats view, and a payload
    // with no fields exercises the missing-value fallbacks.
    assert!(
        crate::cli_read_render::render_text(
            "unknown-verb",
            &json!({"bank": "b"}),
            Palette::new(false)
        )
        .contains("bank: b")
    );
    assert_eq!(
        crate::cli_read_render::render_text("stats", &json!({}), Palette::new(false)),
        "bank: ?\nitems_total: ?\nitems_active: ?\nitems_discarded: ?"
    );
    assert_eq!(
        crate::cli_read_render::render_text("get", &json!({}), Palette::new(false)),
        "id: ?"
    );
    assert_eq!(
        crate::cli_read_render::render_text("inspect", &json!({}), Palette::new(false)),
        "items: 0 of 0"
    );
}

#[test]
fn usage_errors_exit_two() {
    let state = state_with_items();
    let cases: &[(&str, &[&str])] = &[
        ("recall", &[]),
        ("recall", &["a", "b"]),
        ("recall", &["q", "--limit", "abc"]),
        ("recall", &["q", "--time-axis", "bogus"]),
        ("recall", &["q", "--limt", "2"]),
        ("recall", &["q", "--output", "yaml"]),
        ("get", &[]),
        ("get", &["a", "b"]),
        ("show", &[]),
        ("inspect", &["extra"]),
        ("inspect", &["--validity", "bogus"]),
        ("inspect", &["--filter", "not-json"]),
        ("inspect", &["--filter", "[]"]),
        ("inspect", &["--limit", "x"]),
        ("stats", &["extra"]),
        ("bogus", &[]),
    ];
    for &(verb, args) in cases {
        let (code, out, err) = run_state(verb, args, &state, false);
        assert_eq!(code, ExitCode::from(2), "{verb} {args:?}");
        assert!(out.is_empty(), "{verb} {args:?} stdout: {out}");
        assert!(
            err.contains("\"code\":\"usage\""),
            "{verb} {args:?} stderr: {err}"
        );
    }
}

#[test]
fn run_verb_opens_state_and_maps_open_failure() {
    let (code, out, err) = run_open("stats", &["--db", "sqlite::memory:", "--output", "json"]);
    assert_eq!(code, ExitCode::from(0));
    assert!(out.contains("\"ok\":true"), "{out}");
    assert!(err.is_empty(), "stderr: {err}");

    let (code, _out, err) = run_open("recall", &["q", "--backend", "mysql"]);
    assert_eq!(code, ExitCode::from(1));
    assert!(err.contains("\"ok\":false"), "{err}");
}

#[test]
fn tool_failure_adds_the_cross_process_dek_hint() {
    let error = tool_failure("forbidden", "no DEK for subject `itm-1`");
    assert_eq!(error.code(), "forbidden");
    assert!(
        error
            .hint()
            .unwrap_or_default()
            .contains("durable KMS provider")
    );
    let plain = tool_failure("invalid_argument", "item not found");
    assert!(plain.hint().is_none());
}

#[test]
fn tool_errors_mask_credentials() {
    let state = state_with_items();
    let error = call_tool(
        &state,
        "postgres://user:supersecret@db.internal/x",
        &json!({}),
    )
    .expect_err("unknown tool is an error");
    assert_eq!(error.code(), "-32602");
    assert!(error.message().contains("***"), "{}", error.message());
    assert!(
        !error.message().contains("supersecret"),
        "{}",
        error.message()
    );
}

// The score-object tests live in their own child module so both files keep real
// headroom under the 450-line source-file limit. The child reads this module's
// runners and seed fixtures through `use super::*`.

// The per-hit entity tests are a second child module for the same reason, and
// they read the runners and fixtures above through `use super::*`.

// The score-floor and entity-inclusion flag tests are a third child module for
// the same reason, and they read the runners and fixtures above through
// `use super::*`.

// The fake sidecars live in a sibling child module: they are plumbing for the
// score-object tests, not assertions, and moving them out is what leaves the
// assertion file room for a text-view case.
#[cfg(test)]
#[path = "cli_read_tei_doubles_tests.rs"]
mod tei_doubles_tests;

#[cfg(test)]
#[path = "cli_read_tests_entities_tests.rs"]
mod entities_tests;

#[cfg(test)]
#[path = "cli_read_tests_floors_tests.rs"]
mod floors_tests;

#[cfg(test)]
#[path = "cli_read_tests_scores_tests.rs"]
mod scores_tests;
