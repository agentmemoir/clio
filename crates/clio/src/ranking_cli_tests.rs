// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio ranking` commands: reading the effective
//! environment, validated/renormalized weight patches, `--dry-run`, and the
//! text views.
//!
//! ## Owns
//! - Behavior and view tests for the ranking verbs
//!
//! ## Does not own
//! - The command implementation under test (`ranking_cli`)
//! - Ranking weight math (see `clio-config`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::runtime_cli::tests::{run_group, runtime};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

/// Run one `ranking` verb against an injected runtime.
fn run(args: &[&str], runtime: &mut Runtime, is_tty: bool) -> (ExitCode, String, String) {
    run_group(&RankingGroup, "ranking", args, runtime, is_tty)
}

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

/// Per-test scratch directory, removed on drop.
struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("clio-ranking-cli-{}-{seq}", std::process::id()));
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
fn ranking_get_returns_the_effective_environment() {
    let mut rt = runtime();
    let (code, out, err) = run(&["get"], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert!(
        payload["retrieval"]["dense"].as_f64().is_some(),
        "{payload}"
    );
    assert!(payload["admission"]["w1"].as_f64().is_some(), "{payload}");
    assert!(payload["theta_admit"].as_f64().is_some(), "{payload}");

    let (_, text, _) = run(&["get"], &mut rt, true);
    assert!(text.starts_with("ranking environment:"), "{text}");
    assert!(!text.contains("\"ok\""), "{text}");
}

#[test]
fn ranking_set_dry_run_renormalizes_weights_without_applying() {
    let mut rt = runtime();
    let patch = r#"{"retrieval":{"dense":1.0,"lexical":1.0,"importance":1.0}}"#;
    let (code, out, err) = run(&["set", "--patch", patch, "--dry-run"], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["applied"], json!(false), "{payload}");
    let dense = payload["ranking"]["retrieval"]["dense"]
        .as_f64()
        .expect("dense");
    let weights = &payload["ranking"]["retrieval"];
    let sum: f64 = ["dense", "lexical", "importance"]
        .iter()
        .filter_map(|k| weights[*k].as_f64())
        .sum();
    // 1.0 + 1.0 + 1.0 = 3.0, so the patched dense weight is one third.
    assert!((dense - 1.0 / 3.0).abs() < 1e-9, "{payload}");
    assert!(
        (sum - 1.0).abs() < 1e-9,
        "patched weights renormalize: {payload}"
    );

    // The dry-run did not change the effective environment, so this reads the
    // boot default, which is also one third per weight and was never
    // renormalized.
    let (_, after, _) = run(&["get"], &mut rt, false);
    let dense_after = json_of(&after)["retrieval"]["dense"].as_f64().unwrap();
    assert!((dense_after - 1.0 / 3.0).abs() < 1e-9, "{after}");
}

#[test]
fn ranking_set_applies_and_saves_for_later_commands() {
    let (mut rt, path, _dir) = runtime_with_state_file();
    let (code, out, err) = run(
        &["set", "--patch", r#"{"theta_admit":0.6}"#],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["applied"], json!(true), "{payload}");
    assert!((payload["ranking"]["theta_admit"].as_f64().unwrap() - 0.6).abs() < 1e-9);
    assert!(path.is_file(), "a non-dry-run set writes the state file");

    let (_, text, _) = run(&["set", "--patch", r#"{"theta_admit":0.7}"#], &mut rt, true);
    assert!(
        text.contains("applied and saved for later commands"),
        "{text}"
    );
    assert!(!text.contains("lasts only for this process"), "{text}");
}

#[test]
fn a_later_process_boot_reads_the_value_the_cli_saved() {
    let (mut rt, path, _dir) = runtime_with_state_file();
    let (code, _, err) = run(
        &["set", "--patch", r#"{"theta_admit":0.61}"#],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    // The next `clio` run: a fresh runtime booted from the same state file.
    let mut later = Runtime::with_env_and_cli_state_file(&|_key: &str| None, path);
    let (code, out, err) = run(&["get"], &mut later, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        (json_of(&out)["theta_admit"].as_f64().unwrap() - 0.61).abs() < 1e-9,
        "{out}"
    );
}

#[test]
fn a_set_that_cannot_be_saved_is_not_reported_as_applied() {
    // No CLI state path resolves, so nothing can be saved for later commands.
    let mut rt = runtime();
    let (code, out, err) = run(
        &["set", "--patch", r#"{"theta_admit":0.6}"#],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(err.contains("was not saved"), "{err}");
    assert!(!out.contains("applied"), "{out}");
}

#[test]
fn ranking_set_rejects_zero_sum_and_unknown_fields() {
    let mut rt = runtime();
    let zero = r#"{"retrieval":{"dense":0.0,"lexical":0.0,"importance":0.0}}"#;
    let (code, _, err) = run(&["set", "--patch", zero], &mut rt, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("sum"), "{err}");

    let (code, _, err) = run(&["set", "--patch", r#"{"bypass":true}"#], &mut rt, false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("invalid ranking patch"), "{err}");
}

#[test]
fn ranking_set_requires_a_json_object_patch() {
    let mut rt = runtime();
    let (code, _, err) = run(&["set"], &mut rt, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("requires --patch"), "{err}");

    let (code, _, err) = run(&["set", "--patch", "[1,2]"], &mut rt, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("must be a JSON object"), "{err}");

    let (code, _, err) = run(&["set", "--patch", "not-json"], &mut rt, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--patch must be a JSON object"), "{err}");
}

#[test]
fn ranking_set_reads_a_patch_file() {
    let mut rt = runtime();
    let path = std::env::temp_dir().join(format!("clio-ranking-{}.json", std::process::id()));
    std::fs::write(&path, r#"{"theta_admit":0.55}"#).expect("write patch");
    let flag = format!("--patch-file={}", path.display());
    let (code, out, err) = run(&["set", &flag, "--dry-run"], &mut rt, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        (json_of(&out)["ranking"]["theta_admit"].as_f64().unwrap() - 0.55).abs() < 1e-9,
        "{out}"
    );
    let _ = std::fs::remove_file(&path);

    let (code, _, err) = run(
        &["set", "--patch-file", "/nonexistent/patch.json"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("not readable"), "{err}");

    std::fs::write(&path, "not-json").expect("write patch");
    let (code, _, err) = run(&["set", &flag], &mut rt, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--patch-file must be a JSON object"), "{err}");
}

#[test]
fn ranking_set_rejects_extra_positionals() {
    let mut rt = runtime();
    let (code, _, err) = run(&["set", "extra"], &mut rt, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("unexpected extra argument"), "{err}");
}

#[test]
fn render_set_defaults_and_modes() {
    assert_eq!(
        render_set(&json!({"dry_run": true, "ranking": {"a": 1}})),
        "ranking set dry-run (not applied): normalized weights\n{\"a\":1}"
    );
    assert_eq!(
        render_set(&json!({"dry_run": false, "applied": true, "ranking": {"a": 1}})),
        "ranking set applied and saved for later commands: normalized weights\n{\"a\":1}"
    );
    assert_eq!(
        render_set(&json!({"dry_run": false, "applied": false, "ranking": {}})),
        "ranking set not applied: normalized weights\n{}"
    );
    assert_eq!(
        render_set(&json!({"dry_run": true})),
        "ranking set dry-run (not applied): normalized weights\n{}"
    );
}

#[test]
fn ranking_entrypoint_covers_the_process_wrapper() {
    assert_eq!(ranking(vec!["--help".to_owned()]), ExitCode::SUCCESS);
    assert_eq!(
        ranking(vec![
            "get".to_owned(),
            "--output".to_owned(),
            "json".to_owned()
        ]),
        ExitCode::SUCCESS
    );
}
