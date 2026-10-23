// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio` confirmed mutation commands `update`,
//! `invalidate`, `discard`, and `correct`: rule separation and EMA reuse,
//! attributable correction, the off-TTY confirm refusal, and the no-write
//! `--dry-run` previews.
//!
//! ## Owns
//! - Behavior tests for the mutation write verbs
//!
//! ## Does not own
//! - The commands under test (`cli_write_mutate`)
//! - The engine and shared fixtures (see `cli_write` / `cli_write_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{call, json_of, seed_item, state};
use crate::cli_write_test_support::run_write_state;
use serde_json::{Value, json};
use std::process::ExitCode;

/// Store one admitted item in `demo` and return the state.
fn seeded(bank: &str, id: &str, gist: &str) -> std::sync::Arc<clio_mcp::runtime::McpState> {
    let s = state();
    seed_item(&s, bank, id, gist);
    s
}

/// The `stats` counts for `demo`, used to prove writes did or did not happen.
fn stats(s: &clio_mcp::runtime::McpState) -> Value {
    let report = call(s, "stats", json!({"bank": "demo"}));
    assert_eq!(report["ok"], json!(true), "{report}");
    report
}

#[test]
fn update_discrete_stores_the_new_value_on_a_replacement() {
    let s = seeded("demo", "itm-u1", "old value");
    let (code, out, err) = run_write_state(
        "update",
        &[
            "itm-u1",
            "new value",
            "--rule",
            "discrete",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["rule"], json!("discrete"), "{payload}");
    assert!(
        payload["closed_ids"]
            .as_array()
            .is_some_and(|ids| !ids.is_empty()),
        "{payload}"
    );
    let replacement = payload["replacement_id"].as_str().expect("replacement");
    assert!(!replacement.is_empty(), "no replacement: {payload}");
    // The prior is closed and the replacement carrying the new value is active.
    let report = stats(&s);
    assert_eq!(report["items_active"], json!(1), "{report}");
    assert_eq!(report["items_total"], json!(2), "{report}");
}

#[test]
fn update_continuous_reuses_the_shared_ema_engine() {
    let s = state();
    let observed = call(
        &s,
        "persona_observe_preference",
        json!({"key": "verbosity", "observed_value": 0.5, "bank": "demo"}),
    );
    assert_eq!(observed["ok"], json!(true), "{observed}");
    let (code, out, err) = run_write_state(
        "update",
        &["verbosity", "0.8", "--rule", "continuous", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["rule"], json!("continuous"), "{payload}");
    assert!(
        payload
            .pointer("/state/state")
            .and_then(Value::as_f64)
            .is_some(),
        "{payload}"
    );
    // The continuous path mutates the attribute; it must not create an item row.
    assert_eq!(stats(&s)["items_total"], json!(0), "{payload}");
}

#[test]
fn update_rejects_a_rule_outside_discrete_or_continuous() {
    let s = seeded("demo", "itm-u2", "value");
    let (code, out, err) = run_write_state(
        "update",
        &["itm-u2", "1", "--rule", "fact", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("discrete|continuous"), "{err}");
}

#[test]
fn update_requires_the_rule_flag() {
    let s = seeded("demo", "itm-u3", "value");
    let (code, out, err) = run_write_state("update", &["itm-u3", "1"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--rule"), "{err}");
}

#[test]
fn missing_item_mutations_report_one_not_found_code() {
    let s = seeded("demo", "itm-seed", "value");
    for (verb, args) in [
        (
            "update",
            &["itm-nope", "value", "--rule", "discrete", "--bank", "demo"][..],
        ),
        ("invalidate", &["itm-nope", "--bank", "demo"]),
        (
            "discard",
            &[
                "itm-nope",
                "--reason",
                "cleanup",
                "--confirm",
                "--bank",
                "demo",
            ],
        ),
    ] {
        let (code, out, err) = run_write_state(verb, args, &s, false);
        assert_eq!(code, ExitCode::from(1), "{verb}: {out}{err}");
        assert_eq!(
            json_of(&err)["code"],
            json!("item_not_found"),
            "{verb}: {err}"
        );
    }
}

#[test]
fn update_dry_run_writes_nothing() {
    let s = seeded("demo", "itm-u4", "value");
    let (code, out, err) = run_write_state(
        "update",
        &[
            "itm-u4",
            "new",
            "--rule",
            "discrete",
            "--dry-run",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["target"], json!("itm-u4"), "{payload}");
    assert_eq!(stats(&s)["items_active"], json!(1), "no write on dry-run");
}

#[test]
fn invalidate_closes_and_chains_the_replacement() {
    let s = seeded("demo", "itm-i1", "first");
    seed_item(&s, "demo", "itm-i2", "second");
    let (code, out, err) = run_write_state(
        "invalidate",
        &["itm-i1", "--replacement", "itm-i2", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["replacement_id"], json!("itm-i2"), "{payload}");
    assert_eq!(
        payload["closed_ids"].as_array().map_or(0, Vec::len),
        1,
        "{payload}"
    );
}

#[test]
fn invalidate_dry_run_writes_nothing() {
    let s = seeded("demo", "itm-i3", "value");
    let (code, out, err) = run_write_state(
        "invalidate",
        &["itm-i3", "--dry-run", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(stats(&s)["items_active"], json!(1), "no write on dry-run");
}

#[test]
fn correct_records_an_attributable_correction() {
    let s = seeded("demo", "itm-c1", "old gist");
    let (code, out, err) = run_write_state(
        "correct",
        &[
            "itm-c1",
            "corrected gist",
            "--reason",
            "typo fix",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["rule"], json!("discrete"), "{payload}");
    let new_id = payload
        .pointer("/correction/new_item_id")
        .and_then(Value::as_str)
        .expect("new revision id")
        .to_owned();
    assert_eq!(payload["correction"]["prior_item_id"], json!("itm-c1"));
    assert_eq!(payload["correction"]["already_superseded"], json!(false));
    // The correction reason is recorded on the lineage audit trail.
    let trail = call(
        &s,
        "audit_trail",
        json!({"bank": "demo", "item_id": new_id}),
    );
    assert_eq!(trail["ok"], json!(true), "{trail}");
    assert!(trail.to_string().contains("typo fix"), "{trail}");
}

#[test]
fn correct_rejects_a_blank_reason() {
    let s = seeded("demo", "itm-c2", "old gist");
    let (code, out, err) = run_write_state(
        "correct",
        &["itm-c2", "new", "--reason", "", "--bank", "demo"],
        &s,
        false,
    );
    // One rule covers every value flag, so a blank `--reason` is a usage error.
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--reason must not be empty"), "{err}");
}

#[test]
fn correct_dry_run_writes_nothing() {
    let s = seeded("demo", "itm-c3", "old gist");
    let (code, out, err) = run_write_state(
        "correct",
        &[
            "itm-c3",
            "new",
            "--reason",
            "fix",
            "--dry-run",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(
        payload["would_correct"]["item_id"],
        json!("itm-c3"),
        "{payload}"
    );
    assert_eq!(stats(&s)["items_active"], json!(1), "no write on dry-run");
}

#[test]
fn discard_off_tty_without_confirm_refuses_and_writes_nothing() {
    let s = seeded("demo", "itm-d1", "value");
    let (code, out, err) = run_write_state(
        "discard",
        &["itm-d1", "--reason", "noise", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--confirm"), "{err}");
    assert!(err.contains("pass --confirm"), "{err}");
    let report = stats(&s);
    assert_eq!(report["items_discarded"], json!(0), "{report}");
    assert_eq!(report["items_active"], json!(1), "{report}");
}

#[test]
fn discard_with_confirm_sets_the_discarded_columns() {
    let s = seeded("demo", "itm-d2", "value");
    let (code, out, err) = run_write_state(
        "discard",
        &["itm-d2", "--reason", "noise", "--confirm", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["confirm"], json!(true), "{payload}");
    assert_eq!(
        payload["discarded"]["item_id"],
        json!("itm-d2"),
        "{payload}"
    );
    let report = stats(&s);
    assert_eq!(report["items_discarded"], json!(1), "{report}");
    assert_eq!(report["items_active"], json!(0), "{report}");
}

#[test]
fn discard_dry_run_prints_blast_radius_and_writes_nothing() {
    let s = seeded("demo", "itm-d3", "value");
    let (code, out, err) = run_write_state(
        "discard",
        &["itm-d3", "--reason", "noise", "--dry-run", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["operation"], json!("discard"), "{payload}");
    assert_eq!(payload["target"], json!("itm-d3"), "{payload}");
    assert_eq!(payload["scope"], json!("demo"), "{payload}");
    assert_eq!(payload["irreversible"], json!(false), "{payload}");
    assert!(
        payload["would"]
            .as_str()
            .unwrap_or_default()
            .contains("discard"),
        "{payload}"
    );
    let report = stats(&s);
    assert_eq!(report["items_discarded"], json!(0), "{report}");
    assert_eq!(report["items_active"], json!(1), "{report}");
}

#[test]
fn discard_accepts_the_global_yes_alias() {
    let s = seeded("demo", "itm-d4", "value");
    let (code, out, err) = run_write_state(
        "discard",
        &["itm-d4", "--reason", "noise", "--yes", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["confirm"], json!(true), "{payload}");
}

#[test]
fn update_requires_id_and_value_positionals() {
    let s = seeded("demo", "itm-u6", "value");
    let (code, out, err) = run_write_state("update", &["itm-u6", "--rule", "discrete"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("ID VALUE"), "{err}");
}

#[test]
fn correct_requires_id_and_value_positionals() {
    let s = seeded("demo", "itm-c4", "value");
    let (code, out, err) = run_write_state("correct", &["itm-c4", "--reason", "fix"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("ID VALUE"), "{err}");
}

#[test]
fn invalidate_text_mode_view_without_replacement() {
    let s = seeded("demo", "itm-i9", "value");
    let (code, out, err) = run_write_state("invalidate", &["itm-i9", "--bank", "demo"], &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        out.trim(),
        "invalidated (closed 1, already_ended 0) in bank `demo`",
        "{out}"
    );
}

#[test]
fn update_text_mode_view_is_deterministic() {
    let s = seeded("demo", "itm-u5", "value");
    let (code, out, err) = run_write_state(
        "update",
        &["itm-u5", "ignored", "--rule", "discrete", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.starts_with("updated (discrete): closed 1"), "{out}");
}

#[test]
fn discard_text_mode_view_reports_the_removed_item() {
    let s = seeded("demo", "itm-d5", "value");
    let (code, out, err) = run_write_state(
        "discard",
        &["itm-d5", "--reason", "noise", "--confirm", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(out.trim(), "discarded itm-d5 in bank `demo`", "{out}");
}

#[test]
fn discard_dry_run_text_view_states_irreversibility_note() {
    let s = seeded("demo", "itm-d6", "value");
    let (code, out, err) = run_write_state(
        "discard",
        &["itm-d6", "--reason", "noise", "--dry-run", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("would discard itm-d6"), "{out}");
    assert!(out.contains("not compliance erasure"), "{out}");
}
