// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the core `clio` write commands: `remember` success,
//! preview, and refusal; `admit` (single and `--file` batch preview);
//! `summarize`; and `consolidate`, including dry-run no-write guarantees.
//!
//! ## Owns
//! - Behavior and golden tests for `cli_write_core`
//!
//! ## Does not own
//! - The commands under test (`cli_write_core`)
//! - The engine and shared fixtures (see `cli_write` / `cli_write_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{TS, call, json_of, state};
use crate::cli_write_test_support::run_write_state;
use serde_json::json;
use std::process::ExitCode;

fn admit_candidate(id: &str, gist: &str) -> serde_json::Value {
    json!({
        "item": {"id": id, "subject_id": "s", "gist": gist},
        "category": "persona",
        "epistemic_kind": "fact",
        "bank": "demo",
        "timestamp": TS
    })
}

fn candidate_with_category(category: &str) -> serde_json::Value {
    let mut candidate = admit_candidate("itm-gate", "some gist");
    candidate["category"] = json!(category);
    candidate
}

#[test]
fn remember_writes_one_gated_item() {
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "prefers aisle seats",
            "--category",
            "persona",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["pass"], json!(true), "{payload}");
    assert!(payload["admission_score"].as_f64().is_some(), "{payload}");
    assert!(payload["id"].as_str().is_some(), "{payload}");
    // The leaf is queryable before ancestor refresh.
    let id = payload["id"].as_str().expect("stored id");
    assert!(s.leaves.get("demo", id).is_some(), "leaf must be published");
}

#[test]
fn remember_dry_run_writes_nothing_and_exits_zero() {
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "prefers terse answers",
            "--category",
            "persona",
            "--bank",
            "demo",
            "--dry-run",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert!(payload["pass"].is_boolean(), "{payload}");
    assert!(payload["admission_score"].as_f64().is_some(), "{payload}");
    assert!(
        payload["factors"].as_object().is_some(),
        "dry-run prints score and factors: {payload}"
    );
    assert!(
        s.leaves.list_leaves("demo").is_empty(),
        "dry-run must publish nothing: {payload}"
    );
}

#[test]
fn remember_refusal_reports_the_mcp_reason_and_writes_nothing() {
    let s = state();
    // The MCP tool still refuses an unknown category at the taxonomy gate, and
    // still refuses it with a structured message rather than a bare string.
    // The CLI no longer reaches that gate with a bad category: it refuses the
    // flag first, as a usage error (#39), which is asserted in
    // `cli_write_core_category_tests`. What this test keeps is the property
    // that mattered underneath: a refused write publishes nothing, and the
    // domain's refusal is structured.
    let direct = call(&s, "store", candidate_with_category("vibe"));
    let message = direct["message"].as_str().unwrap_or_default();
    assert!(!message.is_empty(), "{direct}");
    assert_eq!(direct["ok"], json!(false), "{direct}");

    // And the CLI's own refusal publishes nothing either.
    let (code, out, err) = run_write_state(
        "remember",
        &["some gist", "--category", "vibe", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(
        s.leaves.list_leaves("demo").is_empty(),
        "refused write stores nothing"
    );
}

#[test]
fn admit_previews_without_writing() {
    let s = state();
    let (code, out, err) = run_write_state(
        "admit",
        &[
            "prefers terse answers",
            "--category",
            "persona",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert!(payload["admission_score"].as_f64().is_some(), "{payload}");
    assert!(
        payload["factors"].as_object().is_some(),
        "admit prints score and factors: {payload}"
    );
}

#[test]
fn admit_batch_file_previews_every_item() {
    let s = state();
    let dir = std::env::temp_dir().join(format!("clio-batch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("ops.json");
    std::fs::write(
        &path,
        json!({
            "items": [
                admit_candidate("itm-1", "prefers terse answers"),
                admit_candidate("itm-2", "hello")
            ]
        })
        .to_string(),
    )
    .expect("write ops.json");
    let arg = format!("--file={}", path.display());
    let (code, out, err) = run_write_state("admit", &[&arg, "--bank", "demo"], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["read_only"], json!(true), "{payload}");
    assert_eq!(payload["count"], json!(2), "{payload}");
    assert_eq!(payload["summary"]["would_admit"], json!(2), "{payload}");
    assert_eq!(payload["summary"]["would_reject"], json!(0), "{payload}");
    assert_eq!(payload["writes"], json!(0), "{payload}");
    assert!(payload["items"].as_array().is_some(), "{payload}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn admit_file_missing_file_exits_two() {
    let s = state();
    let (code, out, err) =
        run_write_state("admit", &["--file", "/nonexistent/ops.json"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("not readable"), "{err}");
}

#[test]
fn admit_file_without_items_array_exits_two() {
    let s = state();
    let dir = std::env::temp_dir().join(format!("clio-batch-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("ops.json");
    std::fs::write(&path, json!({"nope": []}).to_string()).expect("write ops.json");
    let arg = format!("--file={}", path.display());
    let (code, out, err) = run_write_state("admit", &[&arg], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("items"), "{err}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn summarize_requires_scope() {
    let s = state();
    let (code, out, err) = run_write_state("summarize", &[], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--scope"), "{err}");
}

#[test]
fn summarize_dry_run_previews_without_calling_the_dispatcher() {
    let s = state();
    let (code, out, err) =
        run_write_state("summarize", &["--scope", "demo", "--dry-run"], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["scope"], json!("demo"), "{payload}");
    assert!(
        payload["would"]
            .as_str()
            .unwrap_or_default()
            .contains("snapshots are never altered"),
        "{payload}"
    );
}

#[test]
fn consolidate_dry_run_previews_bank_and_force() {
    let s = state();
    let (code, out, err) = run_write_state(
        "consolidate",
        &["--bank", "demo", "--force", "--dry-run"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["force"], json!(true), "{payload}");
    assert_eq!(payload["bank"], json!("demo"), "{payload}");
}

#[test]
fn remember_rejects_unknown_epistemic_kind() {
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &["gist", "--category", "persona", "--epistemic-kind", "vibe"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--epistemic-kind"), "{err}");
}

#[test]
fn remember_then_recall_round_trip_in_one_runtime() {
    // In-process: the flagship write → read loop. Cross-process decryption is
    // the store's process-local dev-KMS limitation.
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "prefers aisle seats",
            "--category",
            "persona",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["pass"], json!(true), "{payload}");
    let (code, out, err) =
        crate::cli_read_test_support::run_state("recall", &["aisle", "--bank", "demo"], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    let hits = payload["hits"].as_array().cloned().unwrap_or_default();
    assert_eq!(hits.len(), 1, "{payload}");
    assert!(hits[0]["item_id"].as_str().is_some(), "{payload}");
}

#[test]
fn text_mode_remember_prints_the_stored_id() {
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "prefers aisle seats",
            "--category",
            "persona",
            "--bank",
            "demo",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("stored itm-cli-"), "{out}");
    assert!(out.contains("admission_score"), "{out}");
}

#[test]
fn text_mode_remember_dry_run_renders_the_preview_not_a_write() {
    // The dry-run payload is a read-only admission decision; the text view
    // must show pass/score/factors and never claim something was stored.
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "prefers aisle seats",
            "--category",
            "persona",
            "--bank",
            "demo",
            "--dry-run",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("preview:"), "{out}");
    assert!(out.contains("score:"), "{out}");
    assert!(out.contains("factors:"), "{out}");
    assert!(
        !out.contains("stored"),
        "dry-run must not claim a write: {out}"
    );
    assert!(s.leaves.list_leaves("demo").is_empty(), "{out}");
}

#[test]
fn text_mode_admit_file_renders_the_batch_summary() {
    let s = state();
    let dir = std::env::temp_dir().join(format!("clio-batch-text-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("ops.json");
    std::fs::write(
        &path,
        json!({
            "items": [
                admit_candidate("itm-1", "prefers aisle seats"),
                admit_candidate("itm-2", "prefers window seats")
            ]
        })
        .to_string(),
    )
    .expect("write ops.json");
    let arg = format!("--file={}", path.display());
    let (code, out, err) = run_write_state("admit", &[&arg, "--bank", "demo"], &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("preview: 2 items"), "{out}");
    assert!(out.contains("would_admit: 2"), "{out}");
    assert!(out.contains("would_reject: 0"), "{out}");
    assert!(out.contains("writes: 0"), "{out}");
    assert!(s.leaves.list_leaves("demo").is_empty(), "{out}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn admit_always_exits_zero_and_reports_the_decision() {
    // `admit` is the pre-flight check: whatever the decision, it is the output
    // (exit 0, zero writes — even for chatter the gates would accept).
    let s = state();
    let (code, out, err) = run_write_state(
        "admit",
        &["hello", "--category", "persona", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert!(payload["pass"].is_boolean(), "{payload}");
    assert!(payload["factors"].as_object().is_some(), "{payload}");
    assert!(s.leaves.list_leaves("demo").is_empty(), "{payload}");
}

#[test]
fn remember_maps_every_optional_flag() {
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "prefers aisle seats",
            "--category",
            "persona",
            "--epistemic-kind",
            "belief",
            "--confidence",
            "0.7",
            "--source-type",
            "user_stated",
            "--evidence-ref",
            "ev-1",
            "--source-text",
            "The user prefers aisle seats on flights.",
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
}

#[test]
fn render_views_fall_back_deterministically() {
    // The fallback arms keep text output stable when an optional field is
    // absent from a tool payload.
    assert_eq!(
        super::render_stored(&json!({"id": "itm-1"})),
        "stored itm-1"
    );
    assert_eq!(super::render_preview(&json!({})), "preview");
    assert_eq!(
        super::render_preview(&json!({"pass": false, "rejection_reason": "nope"})),
        "preview: refused\nreason: nope"
    );
    assert_eq!(
        super::render_job(&json!({"job_id": "j-1", "status": "accepted"})),
        "job j-1 status: accepted"
    );
    assert_eq!(
        super::render_batch_preview(&json!({
            "count": 2,
            "summary": {"would_admit": 1, "would_reject": 1},
            "writes": 0
        })),
        "preview: 2 items (would_admit: 1, would_reject: 1, writes: 0)"
    );
    // Missing optional fields fall back to zero counts, never a write claim.
    assert_eq!(
        super::render_batch_preview(&json!({})),
        "preview: 0 items (would_admit: 0, would_reject: 0, writes: 0)"
    );
}
