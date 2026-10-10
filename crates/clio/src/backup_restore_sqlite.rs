// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite half of `clio backup restore`:
//! measuring the target file, proving room, extracting the archive's members,
//! and placing them with the file swap.
//!
//! ## Owns
//! - `plan_sqlite`: the target measurement, space pre-flight, extraction, and
//!   incoming measurement, in order
//! - `place_sqlite`: the staged payload swap and the key swap
//! - `projected_snapshot_bytes`, the room estimate the pre-restore snapshot
//!   makes
//!
//! ## Does not own
//! - The swap mechanics (see `clio-store`'s `restore_swap`)
//! - The shared plan and report types, refusals, and dispatch (see
//!   `backup_restore_flow`)
//! - The space footprints and the key placement (see `backup_restore_space`)
//! - The Postgres plan and placement (see `backup_restore_postgres`)
//!
//! ## Boundary
//!
//! Changes to the order of a SQLite restore should normally require changes
//! here. Changes to the report shape or the exit codes should not.
//!
//! Keep this module focused on the SQLite restore halves.

use crate::backup_restore_exit::{self, RestoreFailure};
use crate::backup_restore_flow::{
    Counts, Plan, RESULT_PARTIAL, RESULT_RESTORED, Report, RestorePlanInput, pre_restore_decision,
    pre_restore_from,
};
use crate::backup_restore_space::{key_destination, preflight};
use crate::cli_error::CliError;
use clio_store::{SqliteFileState, SwapRequest};
use std::path::{Path, PathBuf};

/// The SQLite plan: measure the target file, prove room, extract, measure the
/// incoming payload.
pub(crate) fn plan_sqlite(input: RestorePlanInput<'_>) -> Result<Plan, CliError> {
    let RestorePlanInput {
        parsed,
        manifest,
        target,
        schema,
        id,
        archive,
        backup_dir,
        check,
    } = input;
    let target_state =
        clio_store::sqlite_file_state(&target.path).map_err(|error| target_unmeasurable(&error))?;
    let target_dir = target_dir(&target.path);
    let projected = projected_snapshot_bytes(&target_state, target.kms_path.as_deref());
    preflight(
        &target_dir,
        backup_dir,
        manifest.payload.bytes,
        manifest.keys.bytes,
        projected,
        check,
    )?;
    let extracted =
        clio_backup::extract_members(archive, &manifest, &target_dir).map_err(|error| {
            backup_restore_exit::from_am_error(
                backup_restore_exit::extraction_failure(&error),
                &error,
            )
        })?;
    // The payload was proved against the manifest's size and digest before it
    // was staged, so a staged payload that will not open as a database is the
    // archive's fault and not the machine's. This is deliberately not routed
    // through `extraction_failure`: that classifier answers "what went wrong
    // while writing a member", and nothing went wrong here.
    let incoming =
        clio_store::sqlite_file_state_unheld(extracted.payload.file.path()).map_err(|error| {
            backup_restore_exit::from_am_error(RestoreFailure::ArchiveDamaged, &error)
        })?;
    let key = key_destination(
        extracted.key.as_ref().map(|member| member.file.path()),
        &target,
    )?;
    let no_pre_backup = parsed.has("no-pre-backup");
    let pre_restore =
        pre_restore_decision(no_pre_backup, target_state.exists, target_state.is_empty());
    Ok(Plan {
        counts_before: Counts::of_state(&target_state),
        counts_incoming: Counts::of_state(&incoming),
        target_exists: target_state.exists,
        created_at: manifest.created_at,
        target,
        backup_dir: backup_dir.to_path_buf(),
        schema,
        dump_meta: None,
        store_schema: None,
        pre_restore,
        no_pre_backup,
        dry_run: parsed.has("dry-run"),
        key,
        extracted,
        snapshot_id: id,
    })
}

/// The SQLite placement: swap the staged payload over the live store, then
/// the staged key store.
pub(crate) fn place_sqlite(plan: Plan) -> Result<Report, CliError> {
    // The skip decision was taken from a measurement made before the operator
    // was asked, and the confirmation prompt waits on a human for as long as
    // they take. So the target is measured again here and the decision is taken
    // again from it: a store that appeared, filled, or emptied while the prompt
    // was up must not inherit a decision from a stale measurement, because
    // skipping the snapshot on that basis replaces rows with no way back.
    let now = clio_store::sqlite_file_state(&plan.target.path)
        .map_err(|error| target_unmeasurable(&error))?;
    let decided = pre_restore_decision(plan.no_pre_backup, now.exists, now.is_empty());
    let pre_restore = pre_restore_from(&plan, decided)?;
    let key_restored = plan.key.is_some();
    let request = SwapRequest {
        staged_payload: plan.extracted.payload.file.path().to_path_buf(),
        target: plan.target.path.clone(),
        key: plan.key,
    };
    let swap = clio_store::swap_sqlite_store(&request).map_err(|error| {
        backup_restore_exit::from_am_error(backup_restore_exit::swap_failure(&error), &error)
    })?;
    let partial = swap.is_partial();
    let key_replaced = key_restored.then_some(swap.key_replaced);
    Ok(Report {
        snapshot_id: plan.snapshot_id,
        target: plan.target,
        pre_restore,
        // The counts the swap is about to replace, not the ones the operator
        // confirmed against: this is the store's state immediately before it.
        counts_before: Counts::of_state(&now),
        counts_after: Some(Counts::of_swap(&swap)),
        counts_incoming: None,
        schema: plan.schema,
        store_schema: plan.store_schema,
        result: if partial {
            RESULT_PARTIAL
        } else {
            RESULT_RESTORED
        },
        failed_step: swap.failed_step.map(ToOwned::to_owned),
        failure: swap.failure,
        dry_run: false,
        key_restored,
        key_replaced,
        directory_synced: Some(swap.directory_synced),
    })
}

/// How many bytes the pre-restore snapshot needs, or zero without a target.
fn projected_snapshot_bytes(target: &SqliteFileState, kms_path: Option<&Path>) -> u64 {
    if !target.exists {
        return 0;
    }
    let keys = kms_path
        .and_then(|path| std::fs::metadata(path).ok())
        .map_or(0, |meta| meta.len());
    target.bytes.saturating_add(keys)
}

/// The target's parent directory, or the current directory for a bare name.
pub(crate) fn target_dir(target: &Path) -> PathBuf {
    target
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// A target store that could not be measured, at plan time or at place time.
///
/// Both halves measure the target and both report the same condition the same
/// way, so the hint names the usual cause rather than only the permissions
/// check: a running `clio` still holding the store is the common one, and a
/// permissions-only hint sends the operator down the wrong path.
fn target_unmeasurable(error: &clio_types::AmError) -> CliError {
    backup_restore_exit::from_am_error(RestoreFailure::TargetUnmeasurable, error)
}

/// The projected pre-restore footprint, with and without a key store.
#[cfg(test)]
#[path = "backup_restore_sqlite_footprint_tests.rs"]
mod footprint_tests;
