// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the pre-restore snapshot's custody record: the row the
//! restore flow writes for the safety copy it takes, and the later restore of
//! that copy the record lets pass the custody gate.
//!
//! ## Owns
//! - The restore-side assertions over the durable store beside the journal,
//!   through the real `restore` command
//!
//! ## Does not own
//! - The create-side custody assertions (see `backup_create_custody_tests`)
//! - The restore route's refusal assertions (see
//!   `backup_restore_custody_route_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the restore stops recording the row for its
//! pre-restore snapshot, or reports a recording failure as success.
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
fn created_snapshot(label: &str, items: usize) -> (Scratch, String) {
    let scratch = Scratch::new(label);
    populated_store(&scratch.join("clio.db"), items, 1);
    ensure_journal(&scratch);
    let (code, payload, err) = run_json_in(&scratch, &["create"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let id = payload["snapshot_id"]
        .as_str()
        .expect("the create produced a snapshot id")
        .to_owned();
    (scratch, id)
}

/// The durable store beside `scratch`'s journal, the one the restore route's
/// lookup reads through.
fn events(scratch: &Scratch) -> SqliteStore {
    let journal = scratch.join(clio_config::ERASURE_JOURNAL_FILE_NAME);
    let kms: Arc<dyn clio_store::DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open(&journal.with_file_name(RESTORE_EVENTS_FILE), kms)
        .expect("open the durable store beside the journal")
}

/// One real restore that takes its pre-restore snapshot, returning the id and
/// archive path the restore published for that safety copy.
fn restored_with_pre_snapshot(label: &str, items: usize) -> (Scratch, String, String, PathBuf) {
    let (scratch, id) = created_snapshot(label, items);
    let (code, payload, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let pre = &payload["pre_restore_snapshot"];
    let pre_id = pre["id"]
        .as_str()
        .expect("the restore recorded its pre-restore snapshot id")
        .to_owned();
    let pre_path = PathBuf::from(pre["path"].as_str().expect("the archive path"));
    (scratch, id, pre_id, pre_path)
}

#[test]
fn a_restore_that_takes_a_pre_restore_snapshot_records_its_custody_row() {
    let (scratch, _, pre_id, pre_path) = restored_with_pre_snapshot("pre-snapshot-custody-row", 2);

    let row = events(&scratch)
        .load(&pre_id)
        .expect("read the custody row")
        .expect("the restore recorded a custody row for its pre-restore snapshot");

    assert_eq!(row.state, "delivered", "{row:?}");
    assert_eq!(
        row.copy_digest.as_deref(),
        Some(copy_digest_of(&std::fs::read(&pre_path).expect("read the archive")).as_str()),
        "the row holds the digest over the archive's bytes"
    );
    assert_eq!(
        row.receiver.as_deref(),
        Some(RESTORE_ACTOR),
        "the receiver is the identity the restore route acts as"
    );
    assert_eq!(row.destination_path, pre_path.display().to_string());
}

#[test]
fn a_restore_of_a_pre_restore_snapshot_passes_the_custody_gate() {
    let (scratch, _, pre_id, _) = restored_with_pre_snapshot("pre-snapshot-restore", 2);

    let (code, _, err) = run_json_in(&scratch, &["restore", &pre_id, "--yes", "--no-pre-backup"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        !err.contains("copy-custody"),
        "the custody gate did not refuse the safety copy: {err}"
    );
}
