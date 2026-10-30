// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the copy-custody record `clio backup create` writes: the
//! row the create records beside the erasure journal, the end-to-end restore
//! that record lets pass, and the failure a create reports when the record
//! cannot be written.
//!
//! ## Owns
//! - The create-side assertions over the durable store beside the journal,
//!   through the real `create` command
//!
//! ## Does not own
//! - The restore route's refusal assertions (see
//!   `backup_restore_custody_route_tests`)
//! - The gate's rules and refusal set (`clio-compliance` owns them)
//!
//! ## Boundary
//!
//! Tests here break when the create stops recording the row the restore route
//! reads, or stops reporting a recording failure.
//!
//! Test-only. Never compiled into production builds.

use crate::backup_cli_fixtures::{Scratch, ensure_journal, populated_store, run_json_in};
use crate::backup_restore_sqlite::{RESTORE_ACTOR, RESTORE_EVENTS_FILE};
use clio_compliance::copy_digest_of;
use clio_store::{DestinationCustodyStore, LocalDevKms, SqliteStore};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

/// A populated store plus one real `clio backup create`, returning the
/// snapshot id it published.
fn created_snapshot(label: &str, items: usize) -> (Scratch, String, PathBuf) {
    let scratch = Scratch::new(label);
    populated_store(&scratch.join("clio.db"), items, 1);
    ensure_journal(&scratch);
    let (code, payload, err) = run_json_in(&scratch, &["create"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let id = payload["snapshot_id"]
        .as_str()
        .expect("the create produced a snapshot id")
        .to_owned();
    let archive = scratch
        .join("backups")
        .join(clio_backup::archive_file_name(&id));
    (scratch, id, archive)
}

/// The durable store beside `scratch`'s journal, the one the restore route's
/// lookup reads through.
fn events(scratch: &Scratch) -> SqliteStore {
    let journal = scratch.join(clio_config::ERASURE_JOURNAL_FILE_NAME);
    let kms: Arc<dyn clio_store::DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open(&journal.with_file_name(RESTORE_EVENTS_FILE), kms)
        .expect("open the durable store beside the journal")
}

#[test]
fn a_create_records_the_custody_row_the_restore_route_reads() {
    let (scratch, id, archive) = created_snapshot("create-custody-row", 2);

    let row = events(&scratch)
        .load(&id)
        .expect("read the custody row")
        .expect("the create recorded a custody row");

    assert_eq!(row.state, "delivered", "{row:?}");
    assert_eq!(
        row.copy_digest.as_deref(),
        Some(copy_digest_of(&std::fs::read(&archive).expect("read the archive")).as_str()),
        "the row holds the digest over the archive's bytes"
    );
    assert_eq!(
        row.receiver.as_deref(),
        Some(RESTORE_ACTOR),
        "the receiver is the identity the restore route acts as"
    );
    assert_eq!(row.destination_path, archive.display().to_string());
}

#[test]
fn a_restore_of_a_create_produced_backup_passes_the_custody_gate() {
    let (scratch, id, _) = created_snapshot("create-custody-restore", 2);

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--no-pre-backup"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        !err.contains("copy-custody"),
        "the custody gate did not refuse: {err}"
    );
}

#[test]
fn a_create_whose_custody_record_cannot_be_written_reports_the_failure() {
    let scratch = Scratch::new("create-custody-failure");
    populated_store(&scratch.join("clio.db"), 2, 1);
    ensure_journal(&scratch);
    // A directory where the durable store must open makes every custody write
    // impossible, without a write helper: the store open is the failure.
    let events_path = scratch
        .join(clio_config::ERASURE_JOURNAL_FILE_NAME)
        .with_file_name(RESTORE_EVENTS_FILE);
    std::fs::create_dir_all(&events_path).expect("block the durable store path");

    let (code, _, err) = run_json_in(&scratch, &["create"]);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(
        err.contains("durable store beside the erasure journal cannot be opened"),
        "the failure names the custody record: {err}"
    );
}
