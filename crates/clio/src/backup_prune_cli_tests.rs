// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the report-only run, the confirmed run, a
//! directory that holds two stores' archives, the help text, and the entry in
//! the `backup` subcommand list.
//!
//! ## Owns
//! - The `prune` behavior assertions: the report-only run, `--dry-run` over
//!   `--confirm`, and the confirmed run
//! - A directory that holds two stores' archives, kept per store
//! - The `--db` and `--backend` refusals, the help text, and the group usage
//!
//! ## Does not own
//! - The keep-count refusals, an unreadable or symlinked archive, and a failed
//!   removal (see `backup_prune_cli_failures_tests`)
//! - The exact JSON shape and the text layout (see `backup_prune_cli_golden_tests`)
//! - The command implementation (`backup_prune_cli`)
//! - Archive creation (see `backup_create_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{
    Scratch, archive_with_manifest, populated_store, run_backup, run_in, run_json_in,
};
use crate::backup_cli_fixtures_handmade::hand_built_archive;
use serde_json::json;
use std::path::PathBuf;

/// The backup directory the runner resolves to.
pub(crate) fn backups(scratch: &Scratch) -> PathBuf {
    scratch.join("backups")
}

/// `count` real archives in the backup directory, one manifest day apart,
/// oldest first, returning their ids in that order.
///
/// The manifest stamp is written by the fixture rather than read from the clock,
/// because two creates can land in the same clock second and a tie is broken by
/// the id, whose collision suffix does not follow the creation order. A prune
/// test that read the time off the clock could not say which archive was newest.
pub(crate) fn timed_archives(scratch: &Scratch, count: usize) -> Vec<String> {
    let store = scratch.join("clio.db");
    populated_store(&store, 2, 1);
    (0..count)
        .map(|index| {
            let id = format!("clio-2026-01-{:02}T000000Z-prune", index + 1);
            let stamp = format!("2026-01-{:02}T00:00:00Z", index + 1);
            hand_built_archive(scratch, &store, &id, false, |manifest| {
                manifest.created_at = stamp;
            });
            id
        })
        .collect()
}

/// The archive ids still on disk in the backup directory, sorted.
pub(crate) fn present(scratch: &Scratch) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(backups(scratch))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| clio_backup::snapshot_id_of(&entry.path()))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// The ids in one of the report's lists, in report order.
pub(crate) fn ids_on(payload: &Value, list: &str) -> Vec<String> {
    payload[list]
        .as_array()
        .unwrap_or_else(|| panic!("`{list}` is a list in the report: {payload}"))
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// `ids` oldest-first, which is the reverse of the order every report list uses.
pub(crate) fn report_order(ids: &[String]) -> Vec<String> {
    ids.iter().rev().cloned().collect()
}

/// The path a report entry names, as a real path.
pub(crate) fn path_of(entry: &Value) -> PathBuf {
    PathBuf::from(entry["path"].as_str().expect("a path"))
}

#[test]
fn a_prune_without_confirmation_reports_the_candidates_and_removes_nothing() {
    let scratch = Scratch::new("prune-report");
    let ids = timed_archives(&scratch, 5);

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "2"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(
        ids_on(&payload, "kept"),
        report_order(&ids[3..]),
        "{payload}"
    );
    assert_eq!(
        ids_on(&payload, "candidates"),
        report_order(&ids[..3]),
        "the three oldest are the candidates: {payload}"
    );
    for candidate in payload["candidates"].as_array().expect("candidates") {
        assert_eq!(candidate["state"], json!("pending"), "{candidate}");
        assert_eq!(candidate["reclaimed_bytes"], json!(0), "{candidate}");
    }
    assert_eq!(
        payload["deleted"].as_array().map(Vec::len),
        Some(0),
        "{payload}"
    );
    assert_eq!(payload["reclaimed_bytes"], json!(0), "{payload}");
    assert_eq!(
        present(&scratch),
        ids,
        "every archive is still there: {payload}"
    );
}

#[test]
fn a_dry_run_wins_over_a_confirmation() {
    let scratch = Scratch::new("prune-dry-run");
    let ids = timed_archives(&scratch, 3);

    let (code, payload, err) = run_json_in(
        &scratch,
        &["prune", "--keep", "1", "--dry-run", "--confirm"],
    );

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["confirmed"], json!(false), "{payload}");
    assert_eq!(
        payload["deleted"].as_array().map(Vec::len),
        Some(0),
        "{payload}"
    );
    assert_eq!(payload["reclaimed_bytes"], json!(0), "{payload}");
    assert_eq!(present(&scratch), ids, "nothing was removed: {payload}");
}

#[test]
fn a_confirmed_prune_removes_the_candidates_and_leaves_the_directory() {
    let scratch = Scratch::new("prune-confirmed");
    let ids = timed_archives(&scratch, 5);

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "2", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["confirmed"], json!(true), "{payload}");
    assert_eq!(payload["dry_run"], json!(false), "{payload}");
    assert_eq!(
        ids_on(&payload, "deleted"),
        report_order(&ids[..3]),
        "{payload}"
    );
    for candidate in payload["candidates"].as_array().expect("candidates") {
        assert_eq!(candidate["state"], json!("deleted"), "{candidate}");
    }
    assert_eq!(
        present(&scratch),
        ids[3..].to_vec(),
        "the two newest survive: {payload}"
    );
    assert!(
        backups(&scratch).is_dir(),
        "the backup directory itself is never removed: {payload}"
    );
}

#[test]
fn a_directory_holding_two_stores_keeps_each_stores_only_archive() {
    let scratch = Scratch::new("prune-two-stores");
    let store = scratch.join("clio.db");
    populated_store(&store, 2, 1);
    let (_sqlite_id, sqlite_path) = archive_with_manifest(&scratch, &store, "sqlite", None, false);
    let (_pg_id, pg_path) = archive_with_manifest(&scratch, &store, "postgres", Some("15"), false);

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "1", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        payload["candidates"].as_array().map(Vec::len),
        Some(0),
        "each archive is the only one of its store, so neither is a candidate: {payload}"
    );
    assert_eq!(
        payload["deleted"].as_array().map(Vec::len),
        Some(0),
        "{payload}"
    );
    assert_eq!(
        payload["kept"].as_array().map(Vec::len),
        Some(2),
        "{payload}"
    );
    assert!(sqlite_path.exists(), "the sqlite archive survived");
    assert!(pg_path.exists(), "the postgres archive survived");
    let (code, out, err) = run_in(&scratch, &["prune", "--keep", "1"], true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        out.contains("sqlite/clio"),
        "the report names the sqlite store: {out}"
    );
    assert!(
        out.contains("postgres/clio"),
        "the report names the postgres store: {out}"
    );
}

#[test]
fn the_non_interactive_flag_is_accepted_and_the_run_is_still_a_report() {
    let scratch = Scratch::new("prune-no-input");
    let ids = timed_archives(&scratch, 2);

    // The command never prompts, so the flag is accepted rather than refused, and
    // accepting it must not turn a missing confirmation into a deletion.
    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "1", "--no-input"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["confirmed"], json!(false), "{payload}");
    assert_eq!(present(&scratch), ids, "{payload}");
}

#[test]
fn prune_refuses_the_store_flags_it_cannot_honor() {
    let scratch = Scratch::new("prune-refuse-store");

    let (code, _, err) = run_in(
        &scratch,
        &["prune", "--keep", "1", "--db", "/tmp/x.db"],
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--db"), "{err}");

    let (code, _, err) = run_in(
        &scratch,
        &["prune", "--keep", "1", "--backend", "postgres"],
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--backend"), "{err}");
}

#[test]
fn prune_help_states_the_three_rules_an_operator_relies_on() {
    let scratch = Scratch::new("prune-help");

    let (code, out, err) = run_in(&scratch, &["prune", "--help"], false);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("usage: clio backup prune"), "{out}");
    // The help wraps its prose, so each claim is matched against the text with
    // its whitespace collapsed.
    let flat = out.split_whitespace().collect::<Vec<_>>().join(" ");
    for claim in [
        "the newest snapshot is never a candidate",
        "reported as unreadable and never deleted",
        "Without --confirm (or with --dry-run) nothing is removed",
    ] {
        assert!(
            flat.contains(claim),
            "the help does not say `{claim}`: {out}"
        );
    }
}

#[test]
fn the_backup_group_usage_lists_every_subcommand() {
    let scratch = Scratch::new("prune-group");
    let (env, cwd) = scratch.targets();

    let (code, out, err) = run_backup(&[], &env.pairs(), Ok(cwd), false);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(out.contains("usage: clio backup <subcommand>"), "{out}");
    for sub in ["create", "dir", "list", "prune", "restore", "verify"] {
        assert!(
            out.contains(&format!("clio backup {sub}")),
            "`{sub}` is missing: {out}"
        );
    }
}

#[test]
fn a_missing_backup_directory_is_an_empty_report_not_a_failure() {
    let scratch = Scratch::new("prune-absent");

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "2", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(
        payload["kept"].as_array().map(Vec::len),
        Some(0),
        "{payload}"
    );
    assert_eq!(
        payload["candidates"].as_array().map(Vec::len),
        Some(0),
        "{payload}"
    );
    assert_eq!(payload["reclaimable_bytes"], json!(0), "{payload}");
    assert_eq!(payload["reclaimed_bytes"], json!(0), "{payload}");
    assert_eq!(payload["remaining_bytes"], json!(0), "{payload}");
    assert!(
        !backups(&scratch).exists(),
        "the report did not create the directory"
    );
}
