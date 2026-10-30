// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite half of `clio backup restore`:
//! measuring the target file, proving room, extracting the archive's members,
//! placing them with the file swap, and the guarded-restore placement plumbing
//! both backend halves share.
//!
//! ## Owns
//! - `plan_sqlite`: the target measurement, space pre-flight, extraction, and
//!   incoming measurement, in order
//! - `place_sqlite`: the staged payload swap and the key swap
//! - `projected_snapshot_bytes`, the room estimate the pre-restore snapshot
//!   makes
//! - `RestorePlacement` and its helpers: the durable restore event store the
//!   guarded restore records into, the action id, the guard begin and settle,
//!   and the guard-refusal mapping
//!
//! ## Does not own
//! - The swap mechanics (see `clio-store`'s `restore_swap`)
//! - The shared plan and report types, refusals, and dispatch (see
//!   `backup_restore_flow`)
//! - The space footprints and the key placement (see `backup_restore_space`)
//! - The authority check itself (see `clio-compliance`'s restore guard)
//! - The Postgres plan and placement (see `backup_restore_postgres`)
//!
//! ## Boundary
//!
//! Changes to the order of a SQLite restore, or to the guarded-restore
//! placement plumbing, should normally require changes here. Changes to the
//! report shape or the exit codes should not.
//!
//! Keep this module focused on the SQLite restore half and the placement guard.

use crate::backup_restore_exit::{self, RestoreFailure};
use crate::backup_restore_flow::{
    Counts, Plan, RESULT_PARTIAL, RESULT_RESTORED, Report, RestorePlanInput, classify_refusal,
    pre_restore_decision, pre_restore_from,
};
use crate::backup_restore_space::{key_destination, preflight};
use crate::cli_error::CliError;
use clio_compliance::{
    CopyCustodyFacts, RestoreGuard, RestoreGuardRefusal, RestoreRequest, begin_restore,
};
use clio_store::{DekProvider, LocalDevKms, LocalEventStore, SqliteFileState, SwapRequest};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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
        journal_path,
        authority,
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
    // The key member is not digest-bound by the manifest, so prove the member
    // this plan will place is the one the authority check classified.
    authority
        .verify_key_member(extracted.key.as_ref())
        .map_err(|refusal| classify_refusal(&refusal))?;
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
        journal_path,
        claim: authority.claim,
        members: authority.members,
        extracted,
        snapshot_id: id,
    })
}

/// The SQLite placement: take the guard, snapshot the live store, then swap the
/// staged payload over it and the staged key store.
pub(crate) fn place_sqlite(
    plan: Plan,
    placement: RestorePlacement<'_>,
) -> Result<Report, CliError> {
    // The skip decision was taken from a measurement made before the operator
    // was asked, and the confirmation prompt waits on a human for as long as
    // they take. So the target is measured again here and the decision is taken
    // again from it: a store that appeared, filled, or emptied while the prompt
    // was up must not inherit a decision from a stale measurement, because
    // skipping the snapshot on that basis replaces rows with no way back.
    let now = clio_store::sqlite_file_state(&plan.target.path)
        .map_err(|error| target_unmeasurable(&error))?;
    let decided = pre_restore_decision(plan.no_pre_backup, now.exists, now.is_empty());
    let key_restored = plan.key.is_some();
    // The guard is taken before the pre-restore snapshot: it takes the journal
    // lock, reconciles any pending restore under it, records the durable intent,
    // and re-checks the authority, so a denial that landed between plan and
    // placement is caught before any snapshot is written. A snapshot failure
    // refuses the guard, so no pending intent is orphaned.
    let guard = placement.begin(&plan)?;
    let pre_restore = match pre_restore_from(&plan, decided) {
        Ok(pre_restore) => pre_restore,
        Err(error) => {
            placement.refuse(guard);
            return Err(error);
        }
    };
    let request = SwapRequest {
        staged_payload: plan.extracted.payload.file.path().to_path_buf(),
        target: plan.target.path.clone(),
        key: plan.key,
    };
    let swap = match clio_store::swap_sqlite_store(&request) {
        Ok(swap) => swap,
        Err(error) => {
            // Nothing was placed, so the guard is settled denied and the lock
            // is released before the refusal is returned.
            placement.refuse(guard);
            return Err(backup_restore_exit::from_am_error(
                backup_restore_exit::swap_failure(&error),
                &error,
            ));
        }
    };
    let partial = swap.is_partial();
    // The store is already replaced, so a failed outcome write leaves a pending
    // action for reconciliation and is surfaced on the report, never returned:
    // the report carries the pre-restore snapshot id, the operator's only way
    // back.
    let settle = placement
        .settle(
            guard,
            true,
            if partial {
                RESULT_PARTIAL
            } else {
                RESULT_RESTORED
            },
        )
        .err();
    let key_replaced = key_restored.then_some(swap.key_replaced);
    let counts_after = Counts::of_swap(&swap);
    let (failed_step, failure) = merge_settle_failure(
        swap.failed_step.map(ToOwned::to_owned),
        swap.failure,
        settle,
    );
    Ok(Report {
        snapshot_id: plan.snapshot_id,
        target: plan.target,
        pre_restore,
        // The counts the swap is about to replace, not the ones the operator
        // confirmed against: this is the store's state immediately before it.
        counts_before: Counts::of_state(&now),
        counts_after: Some(counts_after),
        counts_incoming: None,
        schema: plan.schema,
        store_schema: plan.store_schema,
        result: if partial {
            RESULT_PARTIAL
        } else {
            RESULT_RESTORED
        },
        failed_step,
        failure,
        // Nothing about a file swap derives rows; only a Postgres load can
        // leave a rebuilt table holding a different count.
        derived_difference: None,
        // A swapped-in store file carries its own index, so nothing was emptied.
        derived_index_emptied: false,
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

/// The file name of the durable event store a guarded restore writes to, beside
/// the erasure journal. The copy-custody row lookup reads the same store.
pub(crate) const RESTORE_EVENTS_FILE: &str = "restore-guard-events.db";

/// The actor a guarded restore records: the operator acts for the deployment.
pub(crate) const RESTORE_ACTOR: &str = clio_types::Actor::System.as_str();

/// The outcome text a refused placement records.
const SETTLE_REFUSED: &str = "the restore was refused before placement";

/// The step name reported when the durable outcome write failed after placement.
pub(crate) const FAILED_STEP_SETTLE: &str = "recording the restore outcome";

/// A per-process sequence, so two restores in one process never share an action
/// id, matching the export and review-export id shape.
static RESTORE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The durable action id one guarded restore records.
///
/// The process id and the in-process sequence alone can repeat across a restart
/// with a reused pid, and a repeated id is refused as a duplicate intent, so a
/// legitimate restore would be refused. A random tail from the same random
/// source the encryption key uses makes the id collision-free across restarts.
fn next_restore_action_id() -> String {
    format!(
        "restore-{}-{}-{:016x}",
        std::process::id(),
        RESTORE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        rand::random::<u64>()
    )
}

/// Open the durable restore event store the guarded restore records into.
///
/// It is a SQLite database beside the erasure journal, outside every path a
/// restore replaces, so a pending action survives the placement and a restart
/// can reconcile it.
pub(crate) fn open_restore_events(
    journal_path: &Path,
) -> Result<Arc<dyn LocalEventStore>, CliError> {
    let path = journal_path.with_file_name(RESTORE_EVENTS_FILE);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = clio_store::SqliteStore::open(&path, kms)
        .map_err(|error| CliError::operational(error.code.as_str(), error.message))?;
    Ok(Arc::new(store))
}

/// An `AmError` from the durable event store as a CLI failure.
pub(crate) fn event_error(error: clio_types::AmError) -> CliError {
    CliError::operational(error.code.as_str(), error.message)
}

/// A guard refusal as a restore refusal, keeping its own code.
fn guard_refusal(refusal: &RestoreGuardRefusal) -> CliError {
    CliError::refusal(refusal.code().as_str(), refusal.to_string())
}

/// The guarded-restore placement handle: the durable event store and the event
/// time a restore records. Both backend halves take one guard around their
/// placement.
#[derive(Clone, Copy)]
pub(crate) struct RestorePlacement<'a> {
    pub(crate) events: &'a dyn LocalEventStore,
    pub(crate) now: &'a str,
}

impl RestorePlacement<'_> {
    /// Begin the guard immediately before placement.
    ///
    /// The guard takes the journal lock, reconciles any pending restore action
    /// under it, records the durable intent, and re-checks the authority under
    /// the same hold; the lock is held until the guard settles.
    pub(crate) fn begin(&self, plan: &Plan) -> Result<RestoreGuard, CliError> {
        let action_id = next_restore_action_id();
        // The placement rebuilds the copy-custody facts from the same
        // resolution the plan used, before the lock is taken, so the gate
        // under the lock checks the copy as it exists now.
        let archive = plan
            .backup_dir
            .join(clio_backup::archive_file_name(&plan.snapshot_id));
        let facts = crate::backup_restore_custody::copy_custody_facts(
            &plan.journal_path,
            &plan.snapshot_id,
            &archive,
        )?;
        let request = RestoreRequest {
            journal_path: &plan.journal_path,
            presented: None,
            claim: &plan.claim,
            members: &plan.members,
            copy_custody: CopyCustodyFacts {
                row: facts.row,
                copy_digest: &facts.digest,
                restorer: RESTORE_ACTOR,
            },
            actor: RESTORE_ACTOR,
            action_id: &action_id,
            now: self.now,
        };
        begin_restore(self.events, &request).map_err(|refusal| guard_refusal(&refusal))
    }

    /// Record the placement outcome and release the journal lock.
    pub(crate) fn settle(
        &self,
        guard: RestoreGuard,
        allowed: bool,
        result: &str,
    ) -> Result<(), CliError> {
        guard
            .settle(self.events, allowed, result)
            .map_err(event_error)
    }

    /// Record a refusal and release the lock, ignoring an outcome-write failure:
    /// the refusal is already the caller's error, and a pending row is left for
    /// reconciliation.
    pub(crate) fn refuse(&self, guard: RestoreGuard) {
        let _ = guard.settle(self.events, false, SETTLE_REFUSED);
    }
}

/// Merge a post-placement settle failure into a report's step and failure text.
///
/// The placement already happened, so the failure is surfaced on the report
/// rather than returned; the report is the operator's only record of the
/// pre-restore snapshot. A settle failure leaves a pending action that the next
/// restore reconciles.
pub(crate) fn merge_settle_failure(
    failed_step: Option<String>,
    failure: Option<String>,
    settle: Option<CliError>,
) -> (Option<String>, Option<String>) {
    let Some(error) = settle else {
        return (failed_step, failure);
    };
    let step = failed_step.map_or_else(
        || FAILED_STEP_SETTLE.to_owned(),
        |step| format!("{step}, {FAILED_STEP_SETTLE}"),
    );
    let text = failure.map_or_else(
        || {
            format!(
                "the restore outcome could not be recorded: {}",
                error.message()
            )
        },
        |text| format!("{text}; the restore outcome: {}", error.message()),
    );
    (Some(step), Some(text))
}

/// The projected pre-restore footprint, with and without a key store.
#[cfg(test)]
#[path = "backup_restore_sqlite_footprint_tests.rs"]
mod footprint_tests;
