// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the restore route's
//! copy-custody gate: the facts resolution over the durable store beside the
//! journal, and the refusal mapping onto the CLI's refusal type.
//!
//! ## Owns
//! - The scratch store and archive fixtures for the custody lookup
//! - The absent-row, matching-row, and digest-mismatch gate outcomes through
//!   the route helper
//!
//! ## Does not own
//! - The gate's rules and refusal set (`clio-compliance` owns them)
//! - The route-level restore flow tests (see `backup_restore_flow_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the custody facts resolution or the refusal mapping
//! changes.
//!
//! Test-only. Never compiled into production builds.

use super::{copy_custody_facts, custody_refusal};
use crate::backup_cli_fixtures::Scratch;
use crate::backup_restore_sqlite::{RESTORE_ACTOR, RESTORE_EVENTS_FILE};
use clio_compliance::{RestoreRefusal, copy_digest_of};
use clio_store::{
    DekProvider, DestinationCustodyRow, DestinationCustodyStore, LocalDevKms, SqliteStore,
};
use clio_types::ErrorCode;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The bytes the scratch archive holds, so the recomputed digest is fixed.
const ARCHIVE_BYTES: &[u8] = b"restore-archive-bytes";

/// The digest a mismatching row records, in the delivery expression.
const OTHER_DIGEST: &str =
    "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The snapshot id the fixture rows and lookups share.
const SNAPSHOT: &str = "snap-1";

/// One scratch directory with its journal path and a written archive file.
fn fixture() -> (Scratch, PathBuf, PathBuf) {
    let scratch = Scratch::new("custody-gate");
    let journal = scratch.join("journal.json");
    let archive = scratch.join("copy.archive");
    std::fs::write(&archive, ARCHIVE_BYTES).expect("write the scratch archive");
    (scratch, journal, archive)
}

/// Open the durable store beside the journal the lookup reads through.
fn events(journal: &Path) -> SqliteStore {
    let path = journal.with_file_name(RESTORE_EVENTS_FILE);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open(&path, kms).expect("open the scratch events store")
}

/// A custody row over `digest`, recorded for the snapshot and the restorer.
fn row(digest: &str) -> DestinationCustodyRow {
    DestinationCustodyRow {
        action_id: SNAPSHOT.to_owned(),
        destination_path: "copy".to_owned(),
        custody_owner: "owner".to_owned(),
        required_action: "erase".to_owned(),
        state: "delivered".to_owned(),
        exception_reason: None,
        copy_digest: Some(digest.to_owned()),
        receiver: Some(RESTORE_ACTOR.to_owned()),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

#[test]
fn a_restore_without_a_custody_row_is_refused_as_missing_manifest() {
    let (_scratch, journal, archive) = fixture();
    let error = crate::backup_restore_custody::refuse_without_custody(&journal, SNAPSHOT, &archive)
        .expect_err("no custody row refuses");
    let expected = custody_refusal(RestoreRefusal::CopyManifestMissing);
    assert_eq!(error.code(), expected.code());
    assert_eq!(error.message(), expected.message());
}

#[test]
fn a_restore_whose_row_matches_the_digest_and_receiver_passes() {
    let (_scratch, journal, archive) = fixture();
    let store = events(&journal);
    let digest = copy_digest_of(ARCHIVE_BYTES);
    store.save_pending(&row(&digest)).expect("save the row");
    crate::backup_restore_custody::refuse_without_custody(&journal, SNAPSHOT, &archive)
        .expect("a matching row passes the gate");
}

#[test]
fn a_restore_whose_row_records_a_different_digest_is_refused() {
    let (_scratch, journal, archive) = fixture();
    let store = events(&journal);
    store
        .save_pending(&row(OTHER_DIGEST))
        .expect("save the row");
    let error = crate::backup_restore_custody::refuse_without_custody(&journal, SNAPSHOT, &archive)
        .expect_err("a digest mismatch refuses");
    assert_eq!(error.code(), ErrorCode::Forbidden.as_str());
}

#[test]
fn the_resolved_facts_carry_the_row_and_the_recomputed_digest() {
    let (_scratch, journal, archive) = fixture();
    let facts = copy_custody_facts(&journal, SNAPSHOT, &archive).expect("resolve the facts");
    assert_eq!(facts.row, None);
    assert_eq!(facts.digest, copy_digest_of(ARCHIVE_BYTES));
}
