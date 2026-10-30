// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the restore route's copy-custody refusals through the
//! restore CLI: a restore with no custody row behind the copy, a copy whose
//! recomputed digest differs from the row's, and a restorer that is not the
//! row's receiver all refuse before anything is placed.
//!
//! ## Owns
//! - The three refusal assertions at the route boundary, over a real scratch
//!   store and archive
//!
//! ## Does not own
//! - The gate's rules and refusal set (`clio-compliance` owns them)
//! - The gate's facts resolution (see `backup_restore_custody_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a refusal stops running before placement or its
//! wording loses the operator's next step.
//!
//! Test-only. Never compiled into production builds.

use crate::backup_cli_fixtures::{Scratch, archive_with_manifest, populated_store, run_json_in};
use crate::backup_restore_sqlite::{RESTORE_ACTOR, RESTORE_EVENTS_FILE};
use clio_compliance::copy_digest_of;
use clio_store::{
    DekProvider, DestinationCustodyRow, DestinationCustodyStore, LocalDevKms, SqliteStore,
    sqlite_content_digest,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

/// A digest no archive recomputes to, in the delivery expression's shape.
const OTHER_DIGEST: &str =
    "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// An actor the restore route never acts as.
const OTHER_RECEIVER: &str = "agent";

/// The default target store inside `scratch`.
fn store_path(scratch: &Scratch) -> PathBuf {
    scratch.join("clio.db")
}

/// The durable store beside `scratch`'s journal, the one the route's lookup
/// reads through.
fn events(scratch: &Scratch) -> SqliteStore {
    let journal = scratch.join(clio_config::ERASURE_JOURNAL_FILE_NAME);
    let path = journal.with_file_name(RESTORE_EVENTS_FILE);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open(&path, kms).expect("open the durable store beside the journal")
}

/// Remove the durable store beside the journal, so no custody row backs any
/// copy in `scratch`.
pub(crate) fn clear_copy_custody(scratch: &Scratch) {
    let journal = scratch.join(clio_config::ERASURE_JOURNAL_FILE_NAME);
    let base = journal.with_file_name(RESTORE_EVENTS_FILE);
    for suffix in ["", "-wal", "-shm"] {
        let mut name = base.as_os_str().to_owned();
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }
}

/// Seed a custody row over `archive`, with the digest and receiver a test
/// chooses; `None` keeps the passing value for that field.
pub(crate) fn seed_custody_row(
    scratch: &Scratch,
    id: &str,
    archive: &Path,
    digest: Option<&str>,
    receiver: Option<&str>,
) {
    let store = events(scratch);
    let bytes = std::fs::read(archive).expect("read the archive");
    let row = DestinationCustodyRow {
        action_id: id.to_owned(),
        destination_path: archive.display().to_string(),
        custody_owner: "clio".to_owned(),
        required_action: "delivery".to_owned(),
        state: "delivered".to_owned(),
        exception_reason: None,
        copy_digest: Some(digest.map_or_else(|| copy_digest_of(&bytes), str::to_owned)),
        receiver: Some(receiver.unwrap_or(RESTORE_ACTOR).to_owned()),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    store.save_pending(&row).expect("save the custody row");
}

/// One scratch with a populated store and one restorable archive of it, whose
/// snapshot id comes back with it.
fn fixture(label: &str) -> (Scratch, String) {
    let scratch = Scratch::new(label);
    populated_store(&store_path(&scratch), 2, 1);
    let (id, _) =
        archive_with_manifest(&scratch, &store_path(&scratch), "sqlite", Some("1"), false);
    (scratch, id)
}

/// The archive the fixtures built for `id`.
pub(crate) fn archive_path(scratch: &Scratch, id: &str) -> PathBuf {
    scratch
        .join("backups")
        .join(clio_backup::archive_file_name(id))
}

#[test]
fn a_restore_without_a_custody_row_refuses_before_placement() {
    let (scratch, id) = fixture("restore-custody-missing");
    clear_copy_custody(&scratch);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--allow-missing-keys"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("no copy-custody record"), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before,
        "the target store is byte-identical"
    );
}

#[test]
fn a_restore_whose_row_records_a_different_digest_refuses_before_placement() {
    let (scratch, id) = fixture("restore-custody-digest");
    let archive = archive_path(&scratch, &id);
    clear_copy_custody(&scratch);
    seed_custody_row(&scratch, &id, &archive, Some(OTHER_DIGEST), None);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--allow-missing-keys"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(
        err.contains("digest does not match the digest its custody record holds"),
        "{err}"
    );
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before,
        "the target store is byte-identical"
    );
}

#[test]
fn a_restore_by_an_actor_other_than_the_row_receiver_refuses_before_placement() {
    let (scratch, id) = fixture("restore-custody-receiver");
    let archive = archive_path(&scratch, &id);
    clear_copy_custody(&scratch);
    seed_custody_row(&scratch, &id, &archive, None, Some(OTHER_RECEIVER));
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--allow-missing-keys"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(
        err.contains("not the receiver the copy's custody record names"),
        "{err}"
    );
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before,
        "the target store is byte-identical"
    );
}
