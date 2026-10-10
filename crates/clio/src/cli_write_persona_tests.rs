// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio` persona and history write commands:
//! `persona stable`, `persona observe`, `task upsert`, and `failure record`,
//! including gating parity, EMA updates, `--dry-run` no-write guarantees, and
//! the text views.
//!
//! ## Owns
//! - Behavior tests for the persona/history write verbs
//!
//! ## Does not own
//! - The commands under test (`cli_write_persona`)
//! - The engine and shared fixtures (see `cli_write` / `cli_write_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{TS, call, json_of, state};
use crate::cli_write_group::WriteGroup;
use crate::cli_write_test_support::{run_write_resolved, run_write_state};
use serde_json::json;
use std::process::ExitCode;

#[test]
fn persona_stable_stores_and_round_trips_through_the_dispatcher() {
    let s = state();
    let (code, out, err) = run_write_state(
        "persona stable",
        &[
            "communication_style",
            "concise",
            "--source",
            "explicit",
            "--evidence-ref",
            "ev-1",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["outcome"], json!("admitted"), "{payload}");
    assert_eq!(
        payload["entry"]["key"],
        json!("communication_style"),
        "{payload}"
    );
    assert_eq!(payload["entry"]["value"], json!("concise"), "{payload}");
    assert!(
        payload["decision"]["admission_score"].as_f64().is_some(),
        "{payload}"
    );
    // The stored stable entry is visible through the read dispatcher.
    let doc = call(&s, "persona_get", json!({"bank": "demo", "timestamp": TS}));
    let stable = doc["document"]["stable"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(stable.len(), 1, "{doc}");
    assert_eq!(stable[0]["key"], json!("communication_style"), "{doc}");
    assert_eq!(stable[0]["value"], json!("concise"), "{doc}");
    assert_eq!(stable[0]["source"], json!("explicit"), "{doc}");
}

#[test]
fn persona_stable_text_mode_renders_the_stored_line() {
    let s = state();
    let (code, out, err) = run_write_state(
        "persona stable",
        &["communication_style", "concise", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("stored stable `communication_style`"), "{out}");
    assert!(out.contains("admission_score"), "{out}");
}

#[test]
fn persona_observe_twice_updates_the_ema_state() {
    let s = state();
    let (code, out, err) = run_write_state(
        "persona observe",
        &[
            "brevity_affinity",
            "0.4",
            "--confidence",
            "0.7",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let first = json_of(&out);
    assert_eq!(first["outcome"], json!("observed"), "{first}");
    assert_eq!(first["state"], json!(0.4), "{first}");
    let (code, out, err) = run_write_state(
        "persona observe",
        &["brevity_affinity", "0.9", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let second = json_of(&out);
    assert_ne!(
        second["state"], first["state"],
        "second observation must move the EMA: {second}"
    );
    let doc = call(&s, "persona_get", json!({"bank": "demo", "timestamp": TS}));
    let prefs = doc["document"]["preferences"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(prefs.len(), 1, "observe updates, never duplicates: {doc}");
    assert_eq!(prefs[0]["key"], json!("brevity_affinity"), "{doc}");
}

#[test]
fn persona_observe_rejects_a_non_numeric_value_with_exit_two() {
    let s = state();
    let (code, out, err) = run_write_state(
        "persona observe",
        &["brevity_affinity", "warm", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("NUMBER"), "{err}");
    assert!(
        !err.contains("--observed_value"),
        "there is no `--observed_value` flag, so the hint must not name one: {err}"
    );
}

#[test]
fn task_upsert_dry_run_writes_nothing_and_exit_zero() {
    let s = state();
    let before = call(&s, "stats", json!({"bank": "demo"}));
    let (code, out, err) = run_write_state(
        "task upsert",
        &[
            "task-1",
            "--definition",
            "ship the release",
            "--status",
            "running",
            "--steps",
            "tag, build ,",
            "--bank",
            "demo",
            "--dry-run",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["outcome"], json!("dry_run"), "{payload}");
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    let after = call(&s, "stats", json!({"bank": "demo"}));
    assert_eq!(
        after["items_total"], before["items_total"],
        "{before} {after}"
    );
}

#[test]
fn task_upsert_stores_the_record_and_counts_one_item() {
    let s = state();
    let before = call(&s, "stats", json!({"bank": "demo"}));
    let (code, out, err) = run_write_state(
        "task upsert",
        &[
            "task-1",
            "--definition",
            "ship the release",
            "--status",
            "done",
            "--steps",
            "tag,build",
            "--evidence-ref",
            "ev-2",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["outcome"], json!("stored"), "{payload}");
    assert_eq!(payload["record"]["task_id"], json!("task-1"), "{payload}");
    let after = call(&s, "stats", json!({"bank": "demo"}));
    assert_eq!(
        after["items_total"].as_u64(),
        before["items_total"].as_u64().map(|n| n + 1),
        "{before} {after}"
    );
}

#[test]
fn failure_record_dry_run_writes_nothing_and_stored_counts_one_item() {
    let s = state();
    let before = call(&s, "stats", json!({"bank": "demo"}));
    let args = [
        "task-9",
        "--attempt-n",
        "2",
        "--what-failed",
        "deploy timed out",
        "--lesson",
        "check the health probe first",
        "--bank",
        "demo",
    ];
    let (code, out, err) = {
        let mut with_dry = args.to_vec();
        with_dry.push("--dry-run");
        run_write_state("failure record", &with_dry, &s, false)
    };
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["outcome"], json!("dry_run"), "{payload}");
    let after = call(&s, "stats", json!({"bank": "demo"}));
    assert_eq!(
        after["items_total"], before["items_total"],
        "{before} {after}"
    );
    let (code, out, err) = run_write_state("failure record", &args, &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["outcome"], json!("stored"), "{payload}");
    assert_eq!(payload["record"]["task_id"], json!("task-9"), "{payload}");
    assert_eq!(payload["record"]["attempt_n"], json!(2), "{payload}");
    let after = call(&s, "stats", json!({"bank": "demo"}));
    assert_eq!(
        after["items_total"].as_u64(),
        before["items_total"].as_u64().map(|n| n + 1),
        "{before} {after}"
    );
}

#[test]
fn persona_write_usage_errors_exit_two() {
    let s = state();
    let (code, _out, err) = run_write_state("persona stable", &["only-key"], &s, false);
    assert_eq!(code, ExitCode::from(2));
    assert!(err.contains("persona stable KEY VALUE"), "{err}");
    let (code, _out, err) = run_write_state("persona stable", &["k", "v", "extra"], &s, false);
    assert_eq!(code, ExitCode::from(2));
    assert!(
        err.contains("unexpected extra argument `extra`") || err.contains("extra"),
        "{err}"
    );
    let (code, _out, err) = run_write_state("task upsert", &["task-1", "--status", "s"], &s, false);
    assert_eq!(code, ExitCode::from(2));
    assert!(err.contains("--definition"), "{err}");
    let (code, _out, err) = run_write_state(
        "failure record",
        &["task-1", "--what-failed", "w", "--lesson", "l"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2));
    assert!(err.contains("--attempt-n"), "{err}");
}

#[test]
fn persona_write_zero_argument_invocations_exit_two_without_panicking() {
    let s = state();
    let (code, _out, err) = run_write_state("persona stable", &[], &s, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("persona stable KEY VALUE"), "{err}");
    let (code, _out, err) = run_write_state("persona observe", &[], &s, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("persona observe KEY NUMBER"), "{err}");
}

#[test]
fn dry_run_refusal_mapping_is_none_even_when_the_decision_rejected() {
    let group = super::PersonaWriteGroup;
    let rejected =
        json!({"outcome": "rejected", "decision": {"pass": false, "rejection_reason": "nope"}});
    let refusal = group.refusal("persona stable", &rejected);
    let message = refusal.expect("real rejection maps to a refusal").message;
    assert_eq!(message, "nope");
    // A dry-run payload is a read: never a refusal, even when rejected.
    let dry = json!({
        "outcome": "dry_run",
        "entry": {"key": "k"},
        "decision": {"pass": false, "rejection_reason": "nope"},
        "dry_run": true
    });
    for verb in ["persona stable", "task upsert", "failure record"] {
        assert!(group.refusal(verb, &dry).is_none(), "{verb}");
    }
    // A top-level `dry_run` marker (no `outcome`) is likewise a read.
    assert!(
        group
            .refusal("persona observe", &json!({"dry_run": true, "pass": false}))
            .is_none()
    );
}

#[test]
fn persona_stable_dry_run_happy_path_exit_zero() {
    let s = state();
    let (code, out, err) = run_write_state(
        "persona stable",
        &[
            "communication_style",
            "concise",
            "--bank",
            "demo",
            "--dry-run",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["outcome"], json!("dry_run"), "{payload}");
    let doc = call(&s, "persona_get", json!({"bank": "demo", "timestamp": TS}));
    assert!(
        doc["document"]["stable"]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty(),
        "dry-run stores nothing: {doc}"
    );
}

#[test]
fn persona_stable_help_prints_the_usage_line() {
    let (code, out, err) = run_write_resolved("persona stable", &["--help"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        out.contains("usage: clio persona stable KEY VALUE"),
        "{out}"
    );
    assert!(out.contains("persona_put_stable"), "{out}");
}

#[test]
fn render_views_fall_back_deterministically() {
    // Missing optional fields render as `?`, never a fabricated value.
    assert_eq!(
        super::render_stable(&json!({"outcome": "dry_run", "entry": {"key": "k"}})),
        "dry-run: would upsert stable entry `k`"
    );
    assert_eq!(
        super::render_stable(&json!({"outcome": "admitted", "entry": {"key": "k"}})),
        "stored stable `k`"
    );
    assert_eq!(super::render_stable(&json!({})), "stored stable `?`");
    assert_eq!(
        super::render_observe(&json!({"outcome": "observed", "state": 0.5, "trend": "rising"})),
        "observed `?` state=0.5 trend=rising"
    );
    assert_eq!(
        super::render_observe(&json!({})),
        "observed `?` state=? trend=?"
    );
    assert_eq!(
        super::render_task_upsert(&json!({"outcome": "dry_run", "record": {"task_id": "t"}})),
        "dry-run: would store task `t`"
    );
    assert_eq!(super::render_task_upsert(&json!({})), "stored task `?`");
    assert_eq!(
        super::render_failure_record(&json!({
            "outcome": "dry_run", "record": {"task_id": "t", "attempt_n": 3}
        })),
        "dry-run: would record failure for task `t` (attempt 3)"
    );
    assert_eq!(
        super::render_failure_record(&json!({"record": {"task_id": "t"}})),
        "recorded failure for task `t` (attempt ?)"
    );
}
