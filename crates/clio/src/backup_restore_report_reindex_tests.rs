// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the rebuild line a restore reports: the command that
//! refills the derived index a Postgres restore empties, the fact that gates the
//! line, and the runs that print no such command.
//!
//! ## Owns
//! - The text line and the JSON field, over hand-built reports
//! - The gate: the line appears only when the run's own truncate emptied the
//!   derived index, never because the target's backend is Postgres
//! - That the notice names the embedder the dense leg needs
//! - That the printed command names no store address
//! - That every flag the command carries is one the ops reindex parser accepts
//!
//! ## Does not own
//! - The restore sequence that truncates the table (see `backup_restore_postgres`)
//! - The ops reindex surface the command calls (see `ops_cli`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every expected value here
//! is written in this file or read from the ops registry's own record; none of
//! it comes from the host the test runs on.

use super::{REINDEX_COMMAND, REINDEX_NOTICE, payload, render};
use crate::backup_cli::ResolvedTarget;
use crate::backup_restore_flow::{
    Counts, FAILED_STEP_COUNTS, PreRestore, RESULT_COUNT_MISMATCH, RESULT_PARTIAL, RESULT_RESTORED,
    Report,
};
use crate::backup_restore_schema::{ACTION_NONE, Schema};
use std::path::PathBuf;

/// The marker a target's address field carries here. It is deliberately not a
/// connection URL and not a password: the point of the assertions below is that
/// whatever the address is, it never reaches the report.
const ADDRESS_MARKER: &str = "store-address-marker";

/// A target on the given backend, addressed by `ADDRESS_MARKER`.
fn target(backend: &str) -> ResolvedTarget {
    ResolvedTarget {
        url: ADDRESS_MARKER.to_owned(),
        backend: backend.to_owned(),
        database: "clio".to_owned(),
        display: "clio".to_owned(),
        path: PathBuf::from("/data/clio.db"),
        kms_path: None,
    }
}

/// One placed restore of a store that holds an item, on the given backend, with
/// the given truncation fact.
fn report(backend: &str, dry_run: bool, derived_index_emptied: bool) -> Report {
    Report {
        snapshot_id: "snap-1".to_owned(),
        target: target(backend),
        pre_restore: PreRestore {
            reason: Some("--no-pre-backup was given"),
            id: None,
            path: None,
        },
        counts_before: Counts {
            items: 1,
            triples: 0,
            tables: 3,
            banks: vec!["demo".to_owned()],
        },
        counts_after: Some(Counts {
            items: 1,
            triples: 0,
            tables: 3,
            banks: vec!["demo".to_owned()],
        }),
        counts_incoming: None,
        schema: Schema {
            archive: Some("1".to_owned()),
            action: ACTION_NONE,
        },
        store_schema: None,
        result: RESULT_RESTORED,
        failed_step: None,
        failure: None,
        derived_difference: None,
        derived_index_emptied,
        dry_run,
        key_restored: false,
        key_replaced: None,
        directory_synced: None,
    }
}

/// A Postgres restore truncates the derived vector table and loads nothing back
/// into it, so the report names the command that rebuilds it: one line in the
/// text, one field in the JSON.
#[test]
fn a_placed_postgres_restore_reports_the_rebuild_command() {
    let report = report("postgres", false, true);

    let text = render(&report);
    assert!(
        text.contains(&format!("  rebuild the derived index: {REINDEX_COMMAND}\n")),
        "the command line: {text}"
    );
    assert!(
        text.contains(&format!("  {REINDEX_NOTICE}\n")),
        "the line that says where to point it: {text}"
    );

    let json = payload(&report);
    assert_eq!(
        json["reindex_command"],
        serde_json::json!(REINDEX_COMMAND),
        "{json}"
    );
}

/// The command selects the store on the same backend the restore used and
/// carries no address: `--db` on this backend is a URL that can hold a password,
/// so the report says where to point it rather than repeating it.
#[test]
fn the_rebuild_command_prints_no_store_address() {
    let report = report("postgres", false, true);

    let text = render(&report);
    assert!(!text.contains(ADDRESS_MARKER), "the text: {text}");
    assert!(!text.contains("://"), "no address of any shape: {text}");
    let json = payload(&report).to_string();
    assert!(!json.contains(ADDRESS_MARKER), "the JSON: {json}");
    assert!(!json.contains("://"), "no address of any shape: {json}");
}

/// A backend that keeps its index inside the store it replaced emptied no
/// derived table, so it prints no command and carries no field.
#[test]
fn a_sqlite_restore_reports_no_rebuild_command() {
    let report = report("sqlite", false, false);

    let text = render(&report);
    assert!(!text.contains("reindex"), "the text: {text}");
    let json = payload(&report);
    assert!(
        json.get("reindex_command").is_none(),
        "no field for a backend that keeps its index: {json}"
    );
}

/// A Postgres target whose truncate emptied no derived table keeps its index, so
/// the line is not printed: the backend alone is not the fact the notice claims.
#[test]
fn a_postgres_target_that_emptied_no_derived_table_reports_no_rebuild_command() {
    let report = report("postgres", false, false);

    let text = render(&report);
    assert!(!text.contains("reindex"), "the text: {text}");
    let json = payload(&report);
    assert!(
        json.get("reindex_command").is_none(),
        "no table was emptied, so nothing needs rebuilding: {json}"
    );
}

/// The notice belongs on a run that placed the store and left the index empty
/// even though a user-data count did not match: the rebuild is still owed.
#[test]
fn a_count_mismatch_after_a_placed_restore_still_reports_the_rebuild_command() {
    let mut report = report("postgres", false, true);
    report.result = RESULT_COUNT_MISMATCH;
    report.failed_step = Some(FAILED_STEP_COUNTS.to_owned());
    report.failure = Some("the restored table counts do not match the archive's".to_owned());

    let text = render(&report);
    assert!(
        text.contains(&format!("  rebuild the derived index: {REINDEX_COMMAND}\n")),
        "the index is empty, so the command is still owed: {text}"
    );
    assert_eq!(
        payload(&report)["reindex_command"],
        serde_json::json!(REINDEX_COMMAND),
        "the field travels with the same fact"
    );
}

/// A failed key swap beside a placed store is the same case: the index is empty,
/// so the rebuild is owed and the notice is not withdrawn by the partial result.
#[test]
fn a_failed_key_swap_with_the_store_placed_still_reports_the_rebuild_command() {
    let mut report = report("postgres", false, true);
    report.result = RESULT_PARTIAL;
    report.failed_step = Some(crate::backup_restore_flow::FAILED_STEP_KEY_SWAP.to_owned());
    report.key_restored = true;
    report.key_replaced = Some(false);

    let text = render(&report);
    assert!(
        text.contains(&format!("  rebuild the derived index: {REINDEX_COMMAND}\n")),
        "the index is empty, so the command is still owed: {text}"
    );
}

/// The notice says what the printed command needs: the dense leg fails closed
/// without an embedder, so an operator on a lexical-only install is told before
/// they run it.
#[test]
fn the_rebuild_notice_names_the_embedder_the_dense_leg_needs() {
    let report = report("postgres", false, true);
    let notice = "point it at the restored database: pass the same `--db` this restore used, \
                  with an embedder configured, because the dense leg fails closed without one";

    let text = render(&report);
    assert!(
        text.contains(&format!("  {notice}\n")),
        "the notice the operator reads: {text}"
    );
}

/// A dry run truncates nothing, so it prints no command and carries no field.
#[test]
fn a_postgres_dry_run_reports_no_rebuild_command() {
    let report = report("postgres", true, false);

    let text = render(&report);
    assert!(!text.contains("reindex"), "the text: {text}");
    let json = payload(&report);
    assert!(
        json.get("reindex_command").is_none(),
        "nothing was truncated, so nothing needs rebuilding: {json}"
    );
}

/// Every flag the printed command carries is one the ops reindex record
/// accepts: the command is checked against the parser's own flag list, so a
/// renamed flag cannot leave a command that fails when an operator runs it.
#[test]
fn every_flag_of_the_rebuild_command_is_one_ops_reindex_accepts() {
    let accepted = crate::cli_command_registry_surface::parse_flag_names("ops reindex");

    let flags: Vec<&str> = REINDEX_COMMAND
        .split_whitespace()
        .filter_map(|token| token.strip_prefix("--"))
        .collect();
    assert_eq!(
        flags,
        vec!["target", "confirm", "backend"],
        "the flags the command passes: {REINDEX_COMMAND}"
    );
    for flag in flags {
        assert!(
            accepted.contains(&flag),
            "`--{flag}` is not a flag `ops reindex` parses; it accepts {accepted:?}"
        );
    }
}

/// The target the command rebuilds is the dense leg, and the ops target parser
/// accepts that spelling.
#[test]
fn the_rebuild_command_names_the_dense_leg() {
    assert!(
        REINDEX_COMMAND.contains("--target dense"),
        "the vector table is the dense leg: {REINDEX_COMMAND}"
    );
    assert!(
        matches!(
            clio_index::RebuildTargets::parse("dense"),
            Ok(clio_index::RebuildTargets::Dense)
        ),
        "`dense` is a target the parser accepts"
    );
}
