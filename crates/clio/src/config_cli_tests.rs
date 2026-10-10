// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio config` commands: effective-config reads with
//! masking, validated sets with scope handling, profile listing/apply, and the
//! text views.
//!
//! ## Owns
//! - Behavior and view tests for the config verbs
//!
//! ## Does not own
//! - The command implementation under test (`config_cli`)
//! - Config merge/validation semantics (see `clio-config`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::runtime_cli::tests::{run_group, runtime};
use serde_json::json;

/// Run one `config` verb against an injected runtime.
fn run(args: &[&str], runtime: &mut Runtime, is_tty: bool) -> (ExitCode, String, String) {
    run_group(&ConfigGroup, "config", args, runtime, is_tty)
}

const SECRET: &str = "sk-abcdef0123456789";

#[test]
fn config_get_masks_a_planted_secret_in_path_and_full_views() {
    let mut rt = runtime();
    let (code, _, err) = run(&["set", "credentials.api_key", SECRET], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    let (code, out, err) = run(&["get", "credentials.api_key"], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(!out.contains(SECRET), "{out}");
    assert!(out.contains("****6789"), "{out}");

    // The full effective view masks every secret too.
    let (code, full, err) = run(&["get"], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(!full.contains(SECRET), "{full}");
    assert!(full.contains("****6789"), "{full}");

    let (_, text, _) = run(&["get", "credentials.api_key"], &mut rt, true);
    assert!(!text.contains(SECRET), "{text}");
    assert!(
        text.contains("config credentials.api_key = ****6789"),
        "{text}"
    );
}

#[test]
fn config_set_rejects_invalid_type_and_range_before_apply() {
    let mut rt = runtime();
    let (code, _, err) = run(&["set", "injection.budget_tokens", "0"], &mut rt, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("budget_tokens"), "{err}");

    let (code, _, err) = run(&["set", "log.level", "verbose"], &mut rt, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("log.level"), "{err}");
}

#[test]
fn config_set_rejects_forbidden_and_unknown_paths() {
    let mut rt = runtime();
    let (code, _, err) = run(&["set", "ranking.theta_admit", "0.5"], &mut rt, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("forbidden"), "{err}");

    let (code, _, err) = run(&["set", "nope.path", "1"], &mut rt, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("nope.path"), "{err}");
}

#[test]
fn config_set_profile_scope_requires_an_active_profile() {
    let mut rt = runtime();
    let (code, _, err) = run(
        &["set", "backend", "postgres", "--scope", "profile"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("active profile"), "{err}");
}

#[test]
fn config_set_scope_deployment_updates_the_effective_value() {
    let mut rt = runtime();
    let (code, _, err) = run(
        &["set", "backend", "postgres", "--scope", "deployment"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let (code, out, err) = run(&["get", "backend"], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(json_of(&out)["value"], json!("postgres"), "{out}");
}

#[test]
fn config_set_decodes_json_values_and_falls_back_to_strings() {
    assert_eq!(parse_value("4096"), json!(4096));
    assert_eq!(parse_value("true"), json!(true));
    assert_eq!(parse_value("null"), Value::Null);
    assert_eq!(parse_value("\"quoted\""), json!("quoted"));
    assert_eq!(parse_value("postgres"), json!("postgres"));
    assert_eq!(parse_value("01"), json!("01"));
}

#[test]
fn config_profiles_lists_builtins() {
    let mut rt = runtime();
    let (code, out, err) = run(&["profiles"], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    let names: Vec<&str> = payload["profiles"]
        .as_array()
        .expect("profiles")
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert!(names.contains(&"coding_local"), "{payload}");
    assert!(names.contains(&"coding_ci"), "{payload}");

    let (_, text, _) = run(&["profiles"], &mut rt, true);
    assert!(text.contains("profiles:"), "{text}");
    assert!(text.contains("coding_local"), "{text}");
    assert!(text.contains("single_host=true"), "{text}");
}

#[test]
fn config_use_applies_a_profile_and_reports_no_gate_bypass() {
    let mut rt = runtime();
    let (code, out, err) = run(&["use", "coding_local"], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["name"], json!("coding_local"), "{payload}");
    assert_eq!(
        payload["diff"]["profile.active"]["after"],
        json!("coding_local"),
        "{payload}"
    );

    let (_, text, _) = run(&["use", "coding_ci"], &mut rt, true);
    assert!(text.contains("applied profile coding_ci"), "{text}");
    assert!(text.contains("does not bypass"), "{text}");
}

#[test]
fn config_use_unknown_profile_fails() {
    let mut rt = runtime();
    let (code, _, err) = run(&["use", "nope"], &mut rt, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("unknown profile"), "{err}");
}

#[test]
fn config_get_unknown_path_exits_one() {
    let mut rt = runtime();
    let (code, _, err) = run(&["get", "does.not.exist"], &mut rt, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("unknown config path"), "{err}");
}

#[test]
fn config_extra_positionals_exit_two() {
    let mut rt = runtime();
    let (code, _, err) = run(&["get", "a", "b"], &mut rt, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    let (code, _, err) = run(&["set", "backend"], &mut rt, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    let (code, _, err) = run(&["use"], &mut rt, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
}

#[test]
fn render_get_and_set_default_their_missing_fields() {
    assert_eq!(
        render_get(&json!({"path": "", "value": {"a": 1}, "active_profile": null})),
        "effective configuration (secrets masked; active profile (none))\n{\n  \"a\": 1\n}"
    );
    assert_eq!(
        render_get(&json!({"path": "backend", "value": "sqlite"})),
        "config backend = sqlite"
    );
    assert_eq!(
        render_set(&json!({"path": "backend", "scope": "session", "value": "sqlite"})),
        "config set backend (scope session) = sqlite"
    );
    assert_eq!(
        render_profiles(&json!({"profiles": [], "active": null})),
        "profiles:"
    );
}

#[test]
fn config_entrypoint_covers_the_process_wrapper() {
    assert_eq!(config(vec!["--help".to_owned()]), ExitCode::SUCCESS);
    assert_eq!(config(vec!["bogus".to_owned()]), ExitCode::from(2));
    assert_eq!(config(Vec::<String>::new()), ExitCode::from(2));
}

#[test]
fn render_get_and_set_render_non_string_values_and_active_profiles() {
    assert_eq!(
        render_get(&json!({"path": "x", "value": {"a": 1}})),
        "config x = {\"a\":1}"
    );
    assert!(
        render_get(&json!({"path": "", "value": {}, "active_profile": "coding_local"}))
            .contains("active profile coding_local")
    );
    assert_eq!(
        render_set(&json!({"path": "x", "scope": "deployment", "value": true})),
        "config set x (scope deployment) = true"
    );
}

#[test]
fn render_profiles_covers_absent_profiles_and_the_active_marker() {
    assert_eq!(render_profiles(&json!({})), "profiles:");
    let active =
        render_profiles(&json!({"active": "coding_local", "profiles": [{"name": "coding_local"}]}));
    assert!(
        active.contains("* coding_local (single_host=false, omit_sync=false)"),
        "{active}"
    );
    let missing = render_profiles(&json!({"profiles": [{}]}));
    assert!(missing.contains("? (single_host=false"), "{missing}");
}

#[test]
fn render_use_covers_missing_bank_policy_and_present_diff() {
    assert_eq!(
        render_use(&json!({"name": "p", "note": "n"})),
        "applied profile p (bank policy ?); n"
    );
    let with_diff = render_use(
        &json!({"name": "p", "note": "n", "bank_policy": {"kind": "required"}, "diff": {"a": 1}}),
    );
    assert!(with_diff.contains("\"a\": 1"), "{with_diff}");
}

/// Parse a JSON output line (test helper).
fn json_of(text: &str) -> Value {
    serde_json::from_str(text.trim()).expect("json output")
}
