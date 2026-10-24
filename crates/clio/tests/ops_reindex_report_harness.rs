// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio ops reindex` report the real binary prints:
//! `--confirm` alone runs the live sweep, a `--dry-run` is a report that is
//! never `ok`, and a confirmed dry run is refused (issue 270).
//!
//! ## Owns
//! - The printed `{ok, dry_run, ..._applied}` fields and their exit codes
//! - The refusal of the contradictory `--confirm --dry-run` pair
//!
//! ## Does not own
//! - The swap under test (`ops_cli` / `clio-ops`)
//! - The exit-code mapping itself (`ops_cli_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the `ops reindex` JSON contract changes; they never
//! change it.
//!
//! Keep this module focused on the printed reindex report.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A scratch database plus its backup directory for one scenario.
fn fixture(tag: &str) -> (PathBuf, PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "clio-ops-reindex-report-{tag}-{}-{nanos}",
        std::process::id()
    ));
    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups).expect("temp dir");
    (dir.join("clio.db"), backups)
}

/// The `clio` binary with a cleared environment: only the scenario's backup
/// directory and a scratch key file, never the machine's real ones.
fn clio(backups: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    cmd.env("CLIO_BACKUP_DIR", backups);
    cmd.env("CLIO_KEY_FILE", backups.join("clio.key"));
    cmd.env("CLIO_KEY_FILE_ONLY", "1");
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    cmd
}

/// Run one command against the scenario database with piped streams.
fn run(db: &Path, backups: &Path, args: &[&str]) -> Output {
    clio(backups)
        .args(args)
        .arg("--db")
        .arg(db)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio")
}

/// The command's exit code and its parsed JSON report, with both streams on
/// failure.
fn report_of(output: &Output) -> (Option<i32>, Value) {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let report = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\nstdout: {stdout}\nstderr: {stderr}"));
    (output.status.code(), report)
}

/// Seed two items through the binary, so the sweep has rows to re-derive.
fn seed(db: &Path, backups: &Path) {
    for text in ["alpha deploy note", "beta rollout note"] {
        let output = run(
            db,
            backups,
            &[
                "remember",
                text,
                "--category",
                "tool_config",
                "--output",
                "json",
            ],
        );
        assert!(
            output.status.success(),
            "seed failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `--confirm` alone is the live switch: the rebuild runs, the report says
/// `dry_run:false`, and rows are applied.
#[test]
fn confirm_alone_runs_the_live_rebuild() {
    let (db, backups) = fixture("confirm");
    seed(&db, &backups);
    let output = run(
        &db,
        &backups,
        &["ops", "reindex", "--target", "lexical", "--confirm"],
    );
    let (code, report) = report_of(&output);
    assert_eq!(code, Some(0), "{report}");
    assert_eq!(report["ok"], Value::Bool(true), "{report}");
    assert_eq!(report["dry_run"], Value::Bool(false), "{report}");
    assert_eq!(report["indexable_items"], serde_json::json!(2), "{report}");
    assert_eq!(report["lexical_applied"], serde_json::json!(2), "{report}");
}

/// A `--dry-run` is a report: nothing is applied, so it is not `ok` and the
/// command exits 3.
#[test]
fn dry_run_reports_not_ok_and_applies_nothing() {
    let (db, backups) = fixture("dry-run");
    seed(&db, &backups);
    for flags in [vec!["--dry-run"], vec!["--dry-run", "true"]] {
        let mut args = vec!["ops", "reindex", "--target", "lexical"];
        args.extend(flags);
        let (code, report) = report_of(&run(&db, &backups, &args));
        assert_eq!(code, Some(3), "{report}");
        assert_eq!(report["ok"], Value::Bool(false), "{report}");
        assert_eq!(report["dry_run"], Value::Bool(true), "{report}");
        assert_eq!(report["lexical_applied"], serde_json::json!(0), "{report}");
        assert_eq!(report["dense_applied"], serde_json::json!(0), "{report}");
    }
}

/// `--confirm` with a `--dry-run` asks for two opposite things, so the command
/// refuses with the shared `invalid_argument` path.
#[test]
fn confirm_with_a_dry_run_is_refused() {
    let (db, backups) = fixture("contradiction");
    seed(&db, &backups);
    let output = run(
        &db,
        &backups,
        &["ops", "reindex", "--confirm", "--dry-run", "true"],
    );
    let (code, report) = report_of(&output);
    assert_eq!(code, Some(3), "{report}");
    assert_eq!(report["ok"], Value::Bool(false), "{report}");
    assert_eq!(report["code"], "invalid_argument", "{report}");
}
