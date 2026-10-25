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
use std::sync::atomic::{AtomicU64, Ordering};

/// Run one `config` verb against an injected runtime.
fn run(args: &[&str], runtime: &mut Runtime, is_tty: bool) -> (ExitCode, String, String) {
    run_group(&ConfigGroup, "config", args, runtime, is_tty)
}

const SECRET: &str = "sk-abcdef0123456789";

#[test]
fn config_get_masks_a_planted_secret_in_path_and_full_views() {
    let (mut rt, _dir) = runtime_with_deployment_file();
    let (code, _, err) = run(
        &[
            "set",
            "credentials.api_key",
            SECRET,
            "--scope",
            "deployment",
        ],
        &mut rt,
        false,
    );
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
    let (code, _, err) = run(
        &[
            "set",
            "injection.budget_tokens",
            "0",
            "--scope",
            "deployment",
        ],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("budget_tokens"), "{err}");

    let (code, _, err) = run(
        &["set", "log.level", "verbose", "--scope", "deployment"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("log.level"), "{err}");
}

#[test]
fn config_set_rejects_forbidden_and_unknown_paths() {
    let mut rt = runtime();
    let (code, _, err) = run(
        &["set", "ranking.theta_admit", "0.5", "--scope", "deployment"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("forbidden"), "{err}");

    let (code, _, err) = run(
        &["set", "nope.path", "1", "--scope", "deployment"],
        &mut rt,
        false,
    );
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
    let (mut rt, _dir) = runtime_with_deployment_file();
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
fn config_set_scope_deployment_without_a_resolved_path_exits_one() {
    let mut rt = runtime();
    let (code, out, err) = run(
        &["set", "log.level", "debug", "--scope", "deployment"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(err.contains("no_config_dir"), "{err}");
    assert!(err.contains("not saved"), "{err}");
    assert!(
        !out.contains("debug"),
        "a refused set reports no applied value"
    );
}

#[test]
fn config_set_profile_scope_refuses_a_secret_before_applying_or_writing() {
    let (mut rt, path, _dir) = runtime_with_state_file();
    let (code, _, err) = run(&["use", "coding_local"], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let before = std::fs::read(&path).expect("the profile activation wrote the state file");

    let (code, out, err) = run(
        &["set", "credentials.api_key", SECRET, "--scope", "profile"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(err.contains("invalid_argument"), "{err}");
    assert!(err.contains("secret"), "{err}");
    assert!(err.contains("unsealed CLI state file"), "{err}");
    assert!(err.contains("--scope deployment"), "{err}");
    assert_eq!(
        std::fs::read(&path).expect("state file"),
        before,
        "a refused set writes nothing"
    );

    let (_, got, err) = run(&["get", "credentials.api_key"], &mut rt, false);
    assert!(
        !got.contains(SECRET),
        "a refused set applies nothing: {got}{err}"
    );
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
    let (mut rt, _path, _dir) = runtime_with_state_file();
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

/// A runtime whose CLI state file lives in a fresh scratch directory, so the
/// test never reads or writes the developer's own config directory.
fn runtime_with_state_file() -> (Runtime, std::path::PathBuf, ScratchDir) {
    let dir = ScratchDir::new();
    let path = dir.join("cli-state.json");
    let rt = Runtime::with_env_and_cli_state_file(&|_key: &str| None, path.clone());
    (rt, path, dir)
}

/// The key a test seals under, installed for its own thread only, so no test
/// reaches the machine's key store and none shares a key with another.
const TEST_SEAL_KEY: [u8; clio_config::secret_key::KEY_LEN] =
    [0x5a; clio_config::secret_key::KEY_LEN];

/// A runtime whose deployment overlay lives in a fresh scratch directory, with a
/// test seal key installed so a secret can be saved and read back.
fn runtime_with_deployment_file() -> (Runtime, ScratchDir) {
    clio_config::secret_seal::test_key::use_key(TEST_SEAL_KEY);
    let dir = ScratchDir::new();
    let rt = Runtime::with_env_and_deployment_file(&|_key: &str| None, dir.join("deployment.json"));
    (rt, dir)
}

/// Per-test scratch directory, removed on drop.
struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("clio-config-cli-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    fn join(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn config_use_persists_the_active_profile_a_later_boot_reports() {
    let (mut rt, path, _dir) = runtime_with_state_file();
    let (code, _, err) = run(&["use", "coding_local"], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(path.is_file(), "a profile activation writes the state file");

    // The next `clio` run: a fresh runtime booted from the same state file.
    let mut later = Runtime::with_env_and_cli_state_file(&|_key: &str| None, path.clone());
    let (code, out, err) = run(&["profiles"], &mut later, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(json_of(&out)["active"], json!("coding_local"), "{out}");

    // The persisted profile makes a profile-scoped write acceptable, and that
    // write is itself saved for the command after it.
    let (code, _, err) = run(
        &["set", "log.level", "debug", "--scope", "profile"],
        &mut later,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let mut third = Runtime::with_env_and_cli_state_file(&|_key: &str| None, path);
    let (code, out, err) = run(&["get", "log.level"], &mut third, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(json_of(&out)["value"], json!("debug"), "{out}");
}

#[test]
fn config_set_scope_session_is_refused_and_saves_nothing() {
    let (mut rt, path, _dir) = runtime_with_state_file();
    let (code, out, err) = run(
        &["set", "log.level", "debug", "--scope", "session"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(err.contains("invalid_argument"), "{err}");
    assert!(err.contains("running process only"), "{err}");
    assert!(err.contains("discarded when this command ends"), "{err}");
    assert!(err.contains("cannot be read by a later command"), "{err}");
    assert!(err.contains("--scope deployment"), "{err}");
    assert!(!path.exists(), "a refused session set writes nothing");
    assert!(!out.contains("debug"), "a refusal reports no applied value");

    // The scope the flag falls back to is refused too, so a bare set cannot
    // report a value that dies with this process.
    let (code, _, err) = run(&["set", "log.level", "debug"], &mut rt, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("running process only"), "{err}");
}
