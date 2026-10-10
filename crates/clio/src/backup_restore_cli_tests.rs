// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio backup restore` surface: its flags, the typed
//! confirmation, the dry run, its text and JSON views, and the exit codes,
//! including the partial restore that must not report success.
//!
//! ## Owns
//! - The `restore` surface, view, exit-code, and confirmation assertions
//!
//! ## Does not own
//! - The restore sequence and its refusals (see `backup_restore_flow_tests`)
//! - The command implementation (`backup_restore_cli`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{
    Scratch, env_from, key_store_path, run_backup_with_input, run_in, run_in_with_input,
    run_json_in, run_json_in_with, snapshot, tokens,
};
#[cfg(unix)]
use crate::backup_cli_fixtures_handmade::running_as_root;
use crate::cli_error::EXIT_PARTIAL;
use clio_store::{SpaceCheck, sqlite_content_digest};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The default target store inside `scratch`.
fn store_path(scratch: &Scratch) -> PathBuf {
    scratch.join("clio.db")
}

/// The file names in `dir`, sorted; an absent directory reads as empty.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The published archive names in the scratch backup directory.
fn archives(scratch: &Scratch) -> Vec<String> {
    listing(&scratch.join("backups"))
        .into_iter()
        .filter(|name| name.ends_with(clio_backup::SNAPSHOT_EXTENSION))
        .collect()
}

/// Run a restore through the injected space check, in JSON mode.
///
/// Takes the full command line (`restore` first) and drops the verb, because
/// `run_with_probe` receives the tokens after it.
fn run_probe(
    scratch: &Scratch,
    args: &[&str],
    check: &dyn SpaceCheck,
) -> (ExitCode, String, String) {
    let (env, cwd) = scratch.targets();
    let pairs = env.pairs();
    let get = env_from(&pairs);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::Cursor::new(&[][..]);
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty: false,
            no_color: true,
            env: &get,
        };
        run_with_probe(
            &mut io,
            OutputMode::Json,
            tokens(&args[1..]),
            Ok(cwd),
            check,
        )
    };
    (
        code,
        String::from_utf8(out).expect("utf8 out"),
        String::from_utf8(err).expect("utf8 err"),
    )
}

/// Run a restore on an injected TTY with JSON output and typed stdin.
fn run_json_tty(scratch: &Scratch, args: &[&str], stdin: &[u8]) -> (ExitCode, Value, String) {
    let (env, cwd) = scratch.targets();
    let mut with_mode: Vec<&str> = args.to_vec();
    with_mode.push("--output");
    with_mode.push("json");
    let (code, out, err) = run_backup_with_input(&with_mode, &env.pairs(), Ok(cwd), true, stdin);
    let payload = serde_json::from_str(out.trim()).unwrap_or(Value::String(out.trim().to_owned()));
    (code, payload, err)
}

/// A probe that refuses one directory, so the pre-flight can be exercised
/// without filling a real filesystem.
struct RefuseDir(PathBuf);

impl SpaceCheck for RefuseDir {
    fn can_hold(&self, dir: &Path, _bytes: u64) -> Result<(), AmError> {
        if dir == self.0 {
            return Err(AmError::new(ErrorCode::QuotaExceeded, "no room here"));
        }
        Ok(())
    }
}

/// A probe that allows everything: used when the failure under test happens
/// after the pre-flight.
struct AllowAll;

impl SpaceCheck for AllowAll {
    fn can_hold(&self, _dir: &Path, _bytes: u64) -> Result<(), AmError> {
        Ok(())
    }
}

#[test]
fn a_restore_into_a_missing_target_writes_the_archive_and_reports_the_shape() {
    let scratch = Scratch::new("restore-empty");
    let id = snapshot(&scratch, 4);
    std::fs::remove_file(store_path(&scratch)).expect("remove the target");

    let (code, payload, err) = run_json_in(&scratch, &["restore", "--latest", "--yes"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["snapshot_id"], json!(id), "{payload}");
    assert_eq!(payload["result"], json!("restored"), "{payload}");
    assert_eq!(payload["dry_run"], json!(false), "{payload}");
    assert_eq!(payload["failed_step"], json!(null), "{payload}");
    assert_eq!(payload["failure"], json!(null), "{payload}");
    assert_eq!(
        payload["target"],
        json!({
            "path": store_path(&scratch).display().to_string(),
            "backend": "sqlite",
            "database": "clio.db",
            "schema": null,
        }),
        "{payload}"
    );
    assert_eq!(
        payload["pre_restore_snapshot"]["id"],
        json!(null),
        "{payload}"
    );
    assert_eq!(
        payload["pre_restore_snapshot"]["path"],
        json!(null),
        "{payload}"
    );
    assert_eq!(
        payload["pre_restore_snapshot"]["skipped"],
        json!(true),
        "{payload}"
    );
    assert!(
        payload["pre_restore_snapshot"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("does not exist"),
        "{payload}"
    );
    assert_eq!(
        payload["counts"]["before"],
        json!({"items": 0, "triples": 0, "tables": 0, "banks": []}),
        "{payload}"
    );
    assert_eq!(payload["counts"]["after"]["items"], json!(4), "{payload}");
    assert_eq!(payload["counts"]["incoming"], json!(null), "{payload}");
    assert_eq!(payload["schema"]["archive"], json!("1"), "{payload}");
    assert_eq!(
        payload["schema"]["build"],
        json!(clio_store::SCHEMA_VERSION),
        "{payload}"
    );
    assert_eq!(payload["schema"]["action"], json!("none"), "{payload}");
    assert!(store_path(&scratch).is_file());
}

#[test]
fn a_typed_id_confirms_and_takes_a_pre_restore_snapshot() {
    let scratch = Scratch::new("restore-typed");
    let id = snapshot(&scratch, 3);
    let typed = format!("{id}\n");

    let (code, payload, err) = run_json_tty(&scratch, &["restore", &id], typed.as_bytes());

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["result"], json!("restored"), "{payload}");
    assert_eq!(
        payload["pre_restore_snapshot"]["skipped"],
        json!(false),
        "{payload}"
    );
    let pre_id = payload["pre_restore_snapshot"]["id"]
        .as_str()
        .expect("a pre-restore id");
    assert_ne!(pre_id, id);
    let pre_path = PathBuf::from(
        payload["pre_restore_snapshot"]["path"]
            .as_str()
            .expect("a path"),
    );
    assert!(pre_path.is_file(), "{} should exist", pre_path.display());
    assert_eq!(archives(&scratch).len(), 2);
}

#[test]
fn a_wrong_or_empty_answer_changes_nothing() {
    let scratch = Scratch::new("restore-wrong");
    let id = snapshot(&scratch, 3);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_tty(&scratch, &["restore", &id], b"not-the-id\n");
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("did not match"), "{err}");
    assert!(err.contains("--yes"), "{err}");

    let (code, _, err) = run_json_tty(&scratch, &["restore", &id], b"\n");
    assert_eq!(code, ExitCode::from(2), "{err}");

    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[test]
fn a_non_tty_needs_yes_and_no_input_forces_the_non_interactive_path() {
    let scratch = Scratch::new("restore-tty");
    let id = snapshot(&scratch, 3);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("terminal"), "{err}");
    assert!(err.contains("--yes"), "{err}");

    // A terminal with nothing readable is the same refusal.
    let (code, _, err) = run_json_tty(&scratch, &["restore", &id], b"");
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--yes"), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);

    // --no-input takes the non-interactive path; --yes lets it proceed.
    let (code, payload, err) =
        run_json_tty(&scratch, &["restore", &id, "--no-input", "--yes"], b"");
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["result"], json!("restored"), "{payload}");
}

#[test]
fn a_dry_run_plans_and_changes_nothing() {
    let scratch = Scratch::new("restore-dry");
    let id = snapshot(&scratch, 5);
    let before_digest = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");
    let before_dir = listing(scratch.path());
    let before_backups = listing(&scratch.join("backups"));

    let (code, payload, err) = run_json_in(&scratch, &["restore", &id, "--dry-run", "--yes"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["result"], json!("planned"), "{payload}");
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(
        payload["counts"]["incoming"]["items"],
        json!(5),
        "{payload}"
    );
    assert_eq!(payload["counts"]["after"], json!(null), "{payload}");
    assert_eq!(
        payload["pre_restore_snapshot"]["skipped"],
        json!(false),
        "{payload}"
    );
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before_digest
    );
    assert_eq!(listing(scratch.path()), before_dir);
    assert_eq!(listing(&scratch.join("backups")), before_backups);
}

#[test]
fn the_text_view_names_the_plan_the_prompt_and_the_way_back() {
    let scratch = Scratch::new("restore-text");
    let id = snapshot(&scratch, 2);
    let typed = format!("{id}\r\n");

    let (code, out, err) = run_in_with_input(&scratch, &["restore", &id], true, typed.as_bytes());

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("result: restored"), "{out}");
    assert!(out.contains("pre-restore snapshot:"), "{out}");
    assert!(
        err.contains("restore plan:"),
        "the blast radius goes to stderr: {err}"
    );
    assert!(err.contains("replaced, not merged"), "{err}");
    assert!(err.contains("type the snapshot id"), "{err}");
    assert!(err.contains(&id), "{err}");
}

#[test]
fn the_selector_and_help_rules_fail_closed() {
    let scratch = Scratch::new("restore-selector");
    let id = snapshot(&scratch, 1);

    let (code, out, _) = run_in(&scratch, &["restore", "--help"], false);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("usage: clio backup restore"), "{out}");

    let (code, _, err) = run_json_in(&scratch, &["restore", "--yes"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("needs a snapshot id"), "{err}");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, &id, "--yes"]);
    assert_eq!(code, ExitCode::from(2), "{err}");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--latest", "--yes"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("not both"), "{err}");
}

/// A directory with no room is the machine's condition, not a refusal, so the
/// pre-flight reports it as an operational failure (exit 1). It is detected
/// before the snapshot is taken either way, so nothing is written.
#[test]
fn insufficient_space_fails_before_any_snapshot_with_exit_one() {
    let scratch = Scratch::new("restore-space");
    let id = snapshot(&scratch, 3);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_probe(
        &scratch,
        &["restore", &id, "--yes"],
        &RefuseDir(scratch.join("backups")),
    );

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("pre-restore snapshot"), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[cfg(unix)]
#[test]
fn an_unwritable_backup_directory_fails_the_snapshot_with_exit_one() {
    use std::os::unix::fs::PermissionsExt as _;
    if running_as_root() {
        return;
    }
    let scratch = Scratch::new("restore-unwritable");
    let id = snapshot(&scratch, 3);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");
    let backups = scratch.join("backups");
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o555)).expect("chmod");

    let (code, _, err) = run_probe(&scratch, &["restore", &id, "--yes"], &AllowAll);

    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o700)).expect("chmod back");
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[test]
fn a_bare_relative_target_restores_beside_the_run_directory() {
    let scratch = Scratch::new("restore-relative");
    let id = snapshot(&scratch, 2);
    let (code, payload, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--db", "clio.db"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        payload["target"]["path"],
        json!(store_path(&scratch).display().to_string()),
        "{payload}"
    );
    assert_eq!(payload["counts"]["after"]["items"], json!(2), "{payload}");
}

#[test]
fn a_partial_restore_reports_both_ids_and_exits_three() {
    let scratch = Scratch::new("restore-partial");
    let id = snapshot(&scratch, 3);
    // The live key store is gone and the resolver points into a directory that
    // does not exist: the database swap succeeds, the key rename cannot.
    std::fs::remove_file(key_store_path(&store_path(&scratch))).expect("remove the live key store");
    let missing = scratch.join("missing").join("live.kms.json");
    let missing_str = missing.display().to_string();

    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["restore", &id, "--yes"],
        &[("CLIO_KMS_FILE", missing_str.as_str())],
    );

    assert_eq!(code, ExitCode::from(EXIT_PARTIAL), "{err}");
    assert_eq!(payload["ok"], json!(false), "{payload}");
    assert_eq!(payload["result"], json!("partial"), "{payload}");
    assert_eq!(payload["failed_step"], json!("key-store swap"), "{payload}");
    assert!(
        payload["failure"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "{payload}"
    );
    assert_eq!(payload["snapshot_id"], json!(id), "{payload}");
    assert!(
        payload["pre_restore_snapshot"]["id"].as_str().is_some(),
        "{payload}"
    );
}
