// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the restore route's copy-custody gate: the
//! custody row lookup, the digest recomputed over the archive, the gate call,
//! and the mapping of the copy-custody refusals onto the CLI's refusal type.
//!
//! ## Owns
//! - The plan-path copy-custody refusal and the custody facts the placement
//!   half rebuilds for the under-the-lock re-check
//! - The durable store open the custody row lookup reads through
//!
//! ## Does not own
//! - The gate itself and its refusal set (`clio-compliance` owns
//!   `check_copy_custody` and `RestoreRefusal`)
//! - The shared pre-plan refusal set and the placement sequence (see
//!   `backup_restore_flow` and `backup_restore_sqlite`)
//!
//! ## Boundary
//!
//! Changes to how the custody facts are resolved or how the refusals are
//! worded should normally require changes here. Changes to the gate's rules
//! should not require changes here.
//!
//! Keep this module focused on the copy-custody gate call.

use crate::backup_restore_sqlite::{RESTORE_ACTOR, RESTORE_EVENTS_FILE};
use crate::cli_error::CliError;
use clio_compliance::{CopyCustodyFacts, RestoreRefusal, check_copy_custody};
use clio_store::{
    DekProvider, DestinationCustodyRow, DestinationCustodyStore, LocalDevKms, SqliteStore,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The copy-custody facts one restore resolves: the custody row and the
/// recomputed archive digest.
pub(crate) struct CustodyFacts {
    pub(crate) row: Option<DestinationCustodyRow>,
    pub(crate) digest: String,
}

/// Resolve the copy-custody facts for one restore: the custody row recorded
/// under the snapshot id in the durable store beside the journal, and the
/// digest recomputed over the archive's bytes with the delivery write's
/// expression.
///
/// # Errors
/// The durable store cannot open, the custody row cannot read, or the archive
/// bytes cannot read.
pub(crate) fn copy_custody_facts(
    journal_path: &Path,
    snapshot_id: &str,
    archive: &Path,
) -> Result<CustodyFacts, CliError> {
    let row = custody_row(journal_path, snapshot_id)?;
    let bytes = std::fs::read(archive).map_err(|error| {
        CliError::operational(
            "internal",
            format!("the archive bytes cannot be read: {error}"),
        )
    })?;
    Ok(CustodyFacts {
        row,
        digest: clio_compliance::copy_digest_of(&bytes),
    })
}

/// Refuse a restore whose copy has no matching copy-custody record: no row, a
/// row without a recorded digest, a digest that differs from the recomputed
/// one, or a restorer that is not the recorded receiver.
///
/// # Errors
/// The refusal mapped from the gate, or the custody resolution failure.
pub(crate) fn refuse_without_custody(
    journal_path: &Path,
    snapshot_id: &str,
    archive: &Path,
) -> Result<(), CliError> {
    let facts = copy_custody_facts(journal_path, snapshot_id, archive)?;
    let inputs = CopyCustodyFacts {
        row: facts.row,
        copy_digest: &facts.digest,
        restorer: RESTORE_ACTOR,
    };
    check_copy_custody(&inputs).map_err(custody_refusal)
}

/// The custody row recorded for `action_id` in the durable store beside the
/// journal.
fn custody_row(
    journal_path: &Path,
    action_id: &str,
) -> Result<Option<DestinationCustodyRow>, CliError> {
    let path: PathBuf = journal_path.with_file_name(RESTORE_EVENTS_FILE);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open(&path, kms)
        .map_err(|error| CliError::operational(error.code.as_str(), error.message))?;
    store
        .load(action_id)
        .map_err(|error| CliError::operational(error.code.as_str(), error.message))
}

/// A copy-custody refusal as a clear restore refusal, keeping the gate's own
/// code and text.
pub(crate) fn custody_refusal(refusal: RestoreRefusal) -> CliError {
    let (code, message) = clio_compliance::custody_refusal_parts(refusal);
    CliError::refusal(code.as_str(), message)
}

#[cfg(test)]
#[path = "backup_restore_custody_tests.rs"]
mod tests;
