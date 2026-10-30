// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two archive-content refusals the
//! restore's shared plan half raises before any plan is built: an archive whose
//! backend is not the target's, and an archive that carries no keys.
//!
//! ## Owns
//! - `refuse_backend_mismatch`, the refusal of an archive whose backend is not
//!   the target's, naming both
//! - `refuse_missing_keys`, the refusal of an archive without its keys unless
//!   the operator accepted the loss
//!
//! ## Does not own
//! - The authority and custody gates (see `restore_authority` and
//!   `backup_restore_custody`)
//! - The exit-code table and the condition a refusal maps to (see
//!   `backup_restore_exit`)
//! - The restore's sequence (see `backup_restore_flow`)
//!
//! ## Boundary
//!
//! Changes to what these two refusals refuse, or how they word themselves,
//! should normally require changes here. Changes to the restore's sequence
//! should not.
//!
//! Keep this module focused on the archive-content refusals.

use crate::backup_cli::ResolvedTarget;
use crate::backup_restore_exit::{self, RestoreFailure};
use crate::cli_args::ParsedArgs;
use crate::cli_error::CliError;
use clio_backup::BackupManifest;

/// Refuse an archive whose backend is not the target's, naming both.
pub(crate) fn refuse_backend_mismatch(
    manifest: &BackupManifest,
    target: &ResolvedTarget,
) -> Result<(), CliError> {
    if manifest.store.backend == target.backend {
        return Ok(());
    }
    Err(backup_restore_exit::error(
        RestoreFailure::BackendMismatch,
        "backend_mismatch",
        format!(
            "the archive records the {} backend, but the target store is {}; a restore replaces \
             a store of the same backend and never converts one",
            manifest.store.backend, target.backend
        ),
    )
    .with_hint(format!(
        "restore an archive taken from a {} store",
        target.backend
    )))
}

/// Refuse an archive without its keys unless the operator accepted the loss.
pub(crate) fn refuse_missing_keys(
    manifest: &BackupManifest,
    parsed: &ParsedArgs,
) -> Result<(), CliError> {
    if manifest.keys.included || parsed.has("allow-missing-keys") {
        return Ok(());
    }
    Err(backup_restore_exit::error(
        RestoreFailure::ArchiveWithoutKeys,
        "missing_keys",
        "the archive carries no key store, so the restored content would be permanently \
         unreadable without its keys",
    )
    .with_hint(
        "pass `--allow-missing-keys` only when the keys are kept elsewhere, or restore an \
         archive that carries them",
    ))
}

#[cfg(test)]
#[path = "backup_restore_refusals_tests.rs"]
mod tests;
