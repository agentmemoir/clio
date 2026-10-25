// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres half of `clio backup restore`:
//! planning a Postgres archive's restore against a live Postgres target, and
//! placing it through the store half's one-transaction restore.
//!
//! ## Owns
//! - `plan_postgres`: the target measurement, the dump meta read, the space
//!   pre-flight, the extraction, and the table-set refusal, in that order
//! - `place_postgres`: the one-transaction load, the key swap, and the
//!   post-restore count comparison the report carries
//!
//! ## Does not own
//! - The one-transaction restore itself (see `clio-store`'s postgres restore)
//! - The shared refusal set, confirmation, and pre-restore decision (see
//!   `backup_restore_flow`, which owns the shared plan and report shape)
//! - The space footprints and the key placement (see `backup_restore_space`)
//! - The SQLite plan and placement (see `backup_restore_sqlite`)
//!
//! ## Boundary
//!
//! Changes to the order of a Postgres restore should normally require changes
//! here. Changes to the report shape or the exit codes should not.
//!
//! Keep this module focused on the Postgres restore halves.

use crate::backup_restore_counts::comparison;
use crate::backup_restore_exit::{self, RestoreFailure};
use crate::backup_restore_flow::{
    Counts, FAILED_STEP_COUNTS, FAILED_STEP_KEY_SWAP, Plan, Report, RestorePlanInput,
    classify_refusal, pre_restore_decision, pre_restore_from,
};
use crate::backup_restore_space::{key_destination, preflight};
use crate::backup_restore_sqlite::{RestorePlacement, merge_settle_failure};
use crate::cli_error::CliError;
use clio_backup::dump_meta::DumpMeta;
use clio_store::{PostgresRestoreRequest, RestoreTable, compare_table_sets};
use std::path::Path;

/// The hint a Postgres target that cannot be reached or read carries.
const HINT_TARGET_UNREACHABLE: &str = "check `--db` and the target database's reachability";

/// Plan a Postgres archive's restore, in the shared order: measure the target,
/// read the dump meta, prove room, extract, then refuse a table-set mismatch.
///
/// Every refusal here runs before anything live is touched. The dump meta is
/// read first because the extracted members' space need and the table set the
/// pre-flight compares are its facts. The extraction stages the members into
/// the backup directory, which is owner-only and already holds the archive.
///
/// # Errors
/// The target is unreachable, the archive or its dump meta is damaged, the
/// room check fails, the key member has nowhere to live, or the archive's
/// table set does not match the target's.
pub(crate) fn plan_postgres(input: RestorePlanInput<'_>) -> Result<Plan, CliError> {
    let RestorePlanInput {
        parsed,
        manifest,
        target,
        schema,
        id,
        archive,
        backup_dir,
        check,
        journal_path,
        authority,
    } = input;
    let state = clio_store::postgres_target_state(&target.url)
        .map_err(|error| target_unreachable(&error))?;
    let meta = read_dump_meta(archive)?;
    // The extraction needs exactly the bytes the meta records.
    let dump_bytes = manifest
        .payload
        .bytes
        .saturating_add(meta.table_member_bytes());
    // The pre-restore snapshot is a fresh dump of the target, so its size comes
    // from the target's measured row counts, not from the incoming archive. A
    // small archive restored over a large store must not under-reserve room.
    let snapshot_bytes = projected_snapshot_bytes(&state, &meta);
    preflight(
        backup_dir,
        backup_dir,
        dump_bytes,
        manifest.keys.bytes,
        snapshot_bytes,
        check,
    )?;
    let extracted =
        clio_backup::extract_members(archive, &manifest, backup_dir).map_err(|error| {
            backup_restore_exit::from_am_error(
                backup_restore_exit::extraction_failure(&error),
                &error,
            )
        })?;
    // The key member is not digest-bound by the manifest, so prove the member
    // this plan will place is the one the authority check classified.
    authority
        .verify_key_member(extracted.key.as_ref())
        .map_err(|refusal| classify_refusal(&refusal))?;
    let key = key_destination(
        extracted.key.as_ref().map(|member| member.file.path()),
        &target,
    )?;
    let no_pre_backup = parsed.has("no-pre-backup");
    refuse_table_set_mismatch(&state, &meta)?;
    Ok(Plan {
        counts_before: Counts::of_rows(&state.table_counts, &state.banks),
        counts_incoming: counts_incoming(&meta),
        target_exists: true,
        created_at: manifest.created_at,
        target,
        backup_dir: backup_dir.to_path_buf(),
        schema,
        // The restore loads into the target's own schema: the archive's
        // recorded schema name describes the host the dump came from, and a
        // `--db` whose search_path resolves to another schema must be
        // restored there, never truncated by surprise.
        store_schema: Some(state.schema.clone()),
        dump_meta: Some(meta),
        pre_restore: pre_restore_decision(no_pre_backup, true, state.is_empty()),
        no_pre_backup,
        dry_run: parsed.has("dry-run"),
        key,
        journal_path,
        claim: authority.claim,
        members: authority.members,
        extracted,
        snapshot_id: id,
    })
}

/// Read the archive's dump meta, with the damaged-archive hint the table
/// attaches when the meta really is the archive's fault.
fn read_dump_meta(archive: &Path) -> Result<DumpMeta, CliError> {
    clio_backup::DumpMeta::read_from_archive(archive).map_err(|error| {
        backup_restore_exit::from_am_error(backup_restore_exit::extraction_failure(&error), &error)
    })
}

/// Refuse an archive whose table set does not match the target's.
///
/// The store half runs the same comparison inside its own transaction, so a
/// race between this check and the load cannot empty a target this check
/// cleared; this refusal is the one an operator sees before confirming, and
/// the store half's is the one that holds against the truncate itself.
fn refuse_table_set_mismatch(
    state: &clio_store::PostgresTargetState,
    meta: &DumpMeta,
) -> Result<(), CliError> {
    let target = state
        .tables
        .iter()
        .map(|table| (table.name.clone(), table.column_names()))
        .collect::<Vec<_>>();
    let archive = meta
        .tables
        .iter()
        .map(|table| {
            (
                table.table.clone(),
                table.columns.iter().map(|c| c.name.clone()).collect(),
            )
        })
        .collect::<Vec<_>>();
    compare_table_sets(&target, &archive).map_err(|error| {
        backup_restore_exit::error(
            RestoreFailure::TableSetMismatch,
            "table_set_mismatch",
            error.message,
        )
    })
}

/// A Postgres target that cannot be reached or read. Both halves measure the
/// target and both report it the same way.
fn target_unreachable(error: &clio_types::AmError) -> CliError {
    backup_restore_exit::error(
        RestoreFailure::PostgresTargetRefused,
        error.code.as_str(),
        error.message.clone(),
    )
    .with_hint(HINT_TARGET_UNREACHABLE)
}

/// The incoming counts, from the dump meta's per-table rows.
fn counts_incoming(meta: &DumpMeta) -> Counts {
    let table_counts = meta
        .tables
        .iter()
        .map(|table| (table.table.clone(), table.rows))
        .collect::<Vec<_>>();
    Counts::of_rows(&table_counts, &meta.banks)
}

/// Place a planned Postgres restore: load in one transaction, then swap the
/// key store, then compare the observed counts with the archive's.
///
/// The load and the truncate are one transaction in the store half, so a load
/// failure leaves the target with its original rows. After the commit, the key
/// swap is the step that can still leave a partial restore, and its outcome is
/// reported, never discarded.
///
/// # Errors
/// The pre-restore snapshot fails, the store half's restore refuses or fails,
/// or the measurement after the commit fails.
pub(crate) fn place_postgres(
    plan: Plan,
    placement: RestorePlacement<'_>,
) -> Result<Report, CliError> {
    // The skip decision was taken from a measurement made before the operator
    // was asked, and the confirmation prompt waits on a human for as long as
    // they take. So the target is measured again here and the decision is taken
    // again from it, exactly as the SQLite half does: a store that appeared or
    // was emptied while the prompt was up must not inherit a stale decision.
    let state = clio_store::postgres_target_state(&plan.target.url)
        .map_err(|error| target_unreachable(&error))?;
    let decided = pre_restore_decision(plan.no_pre_backup, true, state.is_empty());
    let key_restored = plan.key.is_some();
    let meta = plan
        .dump_meta
        .as_ref()
        .expect("a Postgres plan carries its dump meta");
    // The one guard is taken before the pre-restore snapshot and held across
    // the load and the separate key swap: it takes the journal lock, reconciles
    // any pending restore under it, records the durable intent, and re-checks
    // the authority, so a denial that landed between plan and placement is
    // caught before the snapshot or the load touches anything. A snapshot
    // failure refuses the guard, so no pending intent is orphaned.
    let guard = placement.begin(&plan)?;
    let pre_restore = match pre_restore_from(&plan, decided) {
        Ok(pre_restore) => pre_restore,
        Err(error) => {
            placement.refuse(guard);
            return Err(error);
        }
    };
    let request = PostgresRestoreRequest {
        url: plan.target.url.clone(),
        // The plan measured the target, so its store schema is the one the
        // load clears and refills.
        schema: plan
            .store_schema
            .clone()
            .expect("a Postgres plan carries its store schema"),
        tables: meta
            .tables
            .iter()
            .zip(plan.extracted.dump_tables.iter())
            .map(|(table, member)| RestoreTable {
                table: table.table.clone(),
                columns: table.columns.iter().map(|c| c.name.clone()).collect(),
                member_path: member.file.path().to_path_buf(),
                rows: table.rows,
            })
            .collect(),
    };
    let restored = match clio_store::restore_postgres(&request) {
        Ok(restored) => restored,
        Err(error) => {
            // The one-transaction load rolled back, so nothing was replaced.
            placement.refuse(guard);
            return Err(backup_restore_exit::from_am_error(
                backup_restore_exit::postgres_load_failure(&error),
                &error,
            ));
        }
    };
    let key_swap = plan
        .key
        .as_ref()
        .map(|key| clio_store::place_staged_key(&key.staged, &key.destination));
    let key_replaced = key_swap.as_ref().map(|report| report.replaced);
    let key_failure = key_swap.as_ref().and_then(|report| report.failure.clone());
    let counts_after = Counts::of_rows(&restored.table_counts, &restored.banks);
    let (result, mismatch, derived_difference) =
        comparison(meta, &restored.table_counts, key_failure.as_deref());
    // The load and the key swap are the effects the guard authorizes. The store
    // is already reloaded, so a failed outcome write leaves a pending action for
    // reconciliation and is surfaced on the report, never returned: the report
    // carries the pre-restore snapshot id, the operator's only way back.
    let settle = placement.settle(guard, true, result).err();
    // Both failures are reported when both happened. A key store that was not
    // placed is a separate problem from a count that did not match, and an
    // operator who reads only one line is sent to the wrong fix.
    let failed_step = match (&mismatch, &key_failure) {
        (Some(_), Some(_)) => Some(format!("{FAILED_STEP_COUNTS}, {FAILED_STEP_KEY_SWAP}")),
        (Some(_), None) => Some(FAILED_STEP_COUNTS.to_owned()),
        (None, Some(_)) => Some(FAILED_STEP_KEY_SWAP.to_owned()),
        (None, None) => None,
    };
    // A derived difference is not a failure: no step failed and nothing was
    // lost, so it travels in its own field and never through `failure`.
    let failure = match (mismatch, key_failure) {
        (Some(counts), Some(key)) => Some(format!("{counts}; the key store: {key}")),
        (Some(counts), None) => Some(counts),
        (None, Some(key)) => Some(key),
        (None, None) => None,
    };
    let (failed_step, failure) = merge_settle_failure(failed_step, failure, settle);
    Ok(Report {
        snapshot_id: plan.snapshot_id,
        target: plan.target,
        pre_restore,
        // The counts the load is about to replace, measured immediately before
        // it, the same way the SQLite half reports them.
        counts_before: Counts::of_rows(&state.table_counts, &state.banks),
        counts_after: Some(counts_after),
        counts_incoming: None,
        schema: plan.schema,
        store_schema: plan.store_schema,
        result,
        failed_step,
        failure,
        derived_difference,
        dry_run: false,
        key_restored,
        key_replaced,
        // No file was renamed, so there is no directory fsync to report.
        directory_synced: None,
    })
}

#[cfg(test)]
#[path = "backup_restore_postgres_tests.rs"]
mod tests;

/// The Postgres restore's error paths: the unreachable target, the damaged
/// dump meta, a member the archive does not carry, a key store with nowhere
/// to land, and a load that fails and rolls back.
#[cfg(test)]
#[path = "backup_restore_postgres_refusal_tests.rs"]
mod refusal_tests;

/// The shared hand-built-archive harness the two test modules above build on.
#[cfg(test)]
#[path = "backup_restore_postgres_assemble_tests.rs"]
mod assemble_tests;

/// The pre-restore snapshot footprint, projected from the target that was
/// just measured rather than from the incoming archive.
///
/// The archive supplies the only bytes-per-row evidence this build has: its own
/// dumped members and the rows they hold. That ratio is applied to the target's
/// measured rows, so a small archive restored over a large store reserves room
/// for the store it is about to snapshot. An archive that declares no rows
/// cannot supply a ratio, so the target's own row total is reserved instead:
/// the check may over-reserve, and it must never under-reserve.
fn projected_snapshot_bytes(state: &clio_store::PostgresTargetState, meta: &DumpMeta) -> u64 {
    let target_rows: u64 = state
        .table_counts
        .iter()
        .map(|(_, count)| *count)
        .fold(0u64, u64::saturating_add);
    if target_rows == 0 {
        return 0;
    }
    let archive_bytes = meta.table_member_bytes();
    let archive_rows: u64 = meta
        .tables
        .iter()
        .map(|table| table.rows)
        .fold(0u64, u64::saturating_add);
    if archive_rows == 0 || archive_bytes == 0 {
        // No ratio to apply: reserve the rows themselves, one byte a row, which
        // is the floor a dump of them could take.
        return target_rows;
    }
    // Ceiling division: reserve target_rows * archive_bytes per archive_rows,
    // rounded up, so the check may over-reserve but never under-reserve.
    let per_row = archive_bytes.div_ceil(archive_rows);
    target_rows.saturating_mul(per_row).max(target_rows)
}

/// The projected pre-restore footprint, including its fallbacks.
#[cfg(test)]
#[path = "backup_restore_postgres_footprint_tests.rs"]
mod footprint_tests;
