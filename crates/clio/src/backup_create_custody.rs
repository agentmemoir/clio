// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the copy-custody record `clio backup create`
//! writes for the archive it publishes.
//!
//! ## Owns
//! - The delivered custody row the create records beside the erasure journal:
//!   keyed by the snapshot id, holding the digest over the archive's bytes and
//!   the receiver the restore route acts as
//!
//! ## Does not own
//! - The archive assembly and publication (see `clio-backup`'s `create`)
//! - The restore route's custody lookup and gate (see `backup_restore_custody`)
//! - The export delivery's custody rows (see `clio-compliance`'s `deliver_export`)
//!
//! ## Boundary
//!
//! Changes to what the create records about a published copy should normally
//! require changes here. Changes to how the restore gate reads a row should
//! not.
//!
//! Keep this module focused on the create's custody record.

use crate::backup_restore_sqlite::{RESTORE_ACTOR, RESTORE_EVENTS_FILE};
use clio_compliance::{CustodyState, copy_digest_of};
use clio_store::{
    DekProvider, DestinationCustodyRow, DestinationCustodyStore, LocalDevKms, SqliteStore,
};
use clio_types::{AmError, ErrorCode};
use std::path::Path;
use std::sync::Arc;

/// Record the delivered copy-custody row for the archive one create published.
///
/// The row is written to the durable store beside the erasure journal — the
/// store the restore route's custody lookup reads — keyed by the snapshot id,
/// holding the digest over the archive's bytes in the delivery write's
/// expression and the receiver the restore route acts as. Without the row, a
/// restore of this archive refuses as a missing custody record, so a create
/// that cannot record it reports the failure rather than publishing a copy no
/// restore can place.
///
/// # Errors
/// The durable store beside the journal cannot open, the archive bytes cannot
/// read, or the row cannot be written.
pub(crate) fn record_delivered_custody(
    journal_path: &Path,
    snapshot_id: &str,
    archive: &Path,
) -> Result<(), AmError> {
    let events = journal_path.with_file_name(RESTORE_EVENTS_FILE);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open(&events, kms).map_err(|cause| {
        AmError::new(
            ErrorCode::Internal,
            format!("the durable store beside the erasure journal cannot be opened: {cause}"),
        )
    })?;
    let bytes = std::fs::read(archive).map_err(|cause| {
        AmError::new(
            ErrorCode::Internal,
            format!("the archive bytes cannot be read: {cause}"),
        )
    })?;
    let row = DestinationCustodyRow {
        action_id: snapshot_id.to_owned(),
        destination_path: archive.display().to_string(),
        custody_owner: "clio".to_owned(),
        required_action: "delivery".to_owned(),
        state: CustodyState::Delivered.as_str().to_owned(),
        exception_reason: None,
        copy_digest: Some(copy_digest_of(&bytes)),
        receiver: Some(RESTORE_ACTOR.to_owned()),
        updated_at: clio_mcp::runtime::now_utc(),
    };
    store.save_pending(&row).map_err(|cause| {
        AmError::new(
            ErrorCode::Internal,
            format!("the copy-custody record for this snapshot cannot be written: {cause}"),
        )
    })
}

#[cfg(test)]
#[path = "backup_create_custody_tests.rs"]
mod tests;
