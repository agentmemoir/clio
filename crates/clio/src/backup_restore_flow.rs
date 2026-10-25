// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the restore flow both backends share:
//! resolving the snapshot and target, running every refusal before anything is
//! written, and dispatching to the per-backend half that plans and places it.
//!
//! ## Owns
//! - The target resolution, the id resolution rule, and the shared refusal set
//!   that runs before either half
//! - The pre-restore decision and the placement-time re-decision
//!
//! ## Does not own
//! - The space pre-flight and the key placement (see `backup_restore_space`)
//! - The ordered plan and placement per backend (see `backup_restore_sqlite`
//!   and `backup_restore_postgres`)
//! - The flags, the prompt, the text and JSON views, and the exit codes (see
//!   `backup_restore_cli` and `backup_restore_report`)
//! - The schema version gate (see `backup_restore_schema`)
//! - Manifest reading, extraction, the swap, and target resolution (see
//!   `clio-backup`, `clio-store`, and `backup_cli`)
//!
//! ## Boundary
//!
//! Changes to the refusals, the dispatch, or the shared plan shape should
//! normally require changes here. Changes to rendering or exit codes should not.

use crate::backup_cli::{
    ResolvedTarget, resolve_backup_dir, resolve_restore_id, resolve_restore_target,
};
use crate::backup_create_cli::take_snapshot;
use crate::backup_restore_exit::{self, RestoreFailure};
use crate::backup_restore_postgres::{place_postgres, plan_postgres};
use crate::backup_restore_schema::{Schema, compare_schema};
use crate::backup_restore_sqlite::{RestorePlacement, open_restore_events};
use crate::cli_args::ParsedArgs;
use crate::cli_error::CliError;
use crate::restore_authority::{ClassifiedArchive, ClassifyRefusal, classify_archive};
use clio_backup::{BackupManifest, ExtractedArchive};
use clio_compliance::{ArchiveClaim, AuthorityRefusal, JournalHold, MemberFacts};
use clio_store::{KeyPlacement, SpaceCheck, SqliteFileState, SwapReport};
use std::path::{Path, PathBuf};

// The result and failed-step words live in one module of their own; they are
// re-exported here so the halves and the views keep importing them from this
// module and no two of them can spell an outcome differently.
pub(crate) use crate::backup_restore_result::{
    FAILED_STEP_COUNTS, FAILED_STEP_KEY_SWAP, RESULT_COUNT_MISMATCH, RESULT_DERIVED_DIFFERENT,
    RESULT_PARTIAL, RESULT_PLANNED, RESULT_RESTORED,
};
// The hints the restore's refusals carry. Which hint a failure gets is decided
// by the exit-code table from the condition, not chosen at the call site.
//
// A hint is backend-neutral: it names no path, pid, credential, or content, and
// the message body beside it names the real path. So the staging hint says
// "the directory the staged member goes in" rather than "the target's
// directory": that directory is the one the run actually stages into, which
// differs by backend — a SQLite restore stages in the target's own directory, a
// Postgres restore stages in the backup directory and has no target directory on
// disk — so only the body can tell the operator which one it was.
pub(crate) const HINT_TARGET_UNMEASURABLE: &str =
    "check `--db`, the file's permissions, and whether another `clio` still holds the store";
pub(crate) const HINT_ARCHIVE_DAMAGED: &str =
    "the archive is damaged; restore a different snapshot";
pub(crate) const HINT_STAGING_FAILED: &str =
    "the directory the staged member goes in could not hold it; free space there and retry";
pub(crate) const SKIP_NO_PRE_BACKUP: &str = "--no-pre-backup was given";
pub(crate) const SKIP_TARGET_MISSING: &str =
    "the target store does not exist yet, so there is nothing to preserve";
pub(crate) const SKIP_TARGET_EMPTY: &str =
    "the target store is empty, so there is nothing to preserve";

/// One set of row counts: the `items` and `triples` rows, the table count,
/// and the banks the `items` rows name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) items: u64,
    pub(crate) triples: u64,
    pub(crate) tables: usize,
    pub(crate) banks: Vec<String>,
}

impl Counts {
    /// The counts of a measured store file.
    pub(crate) fn of_state(state: &SqliteFileState) -> Self {
        Self {
            items: state.count_of("items").unwrap_or(0),
            triples: state.count_of("triples").unwrap_or(0),
            tables: state.table_count(),
            banks: state.banks.clone(),
        }
    }
    /// The counts of a finished swap.
    pub(crate) fn of_swap(report: &SwapReport) -> Self {
        Self {
            items: report.count_of("items").unwrap_or(0),
            triples: report.count_of("triples").unwrap_or(0),
            tables: report.table_counts.len(),
            banks: report.banks.clone(),
        }
    }
    /// The counts of a measured Postgres target or a restored store.
    pub(crate) fn of_rows(table_counts: &[(String, u64)], banks: &[String]) -> Self {
        Self {
            items: table_counts
                .iter()
                .find(|(name, _)| name == "items")
                .map_or(0, |(_, count)| *count),
            triples: table_counts
                .iter()
                .find(|(name, _)| name == "triples")
                .map_or(0, |(_, count)| *count),
            tables: table_counts.len(),
            banks: banks.to_vec(),
        }
    }
}

/// The pre-restore snapshot: the decision in a plan, the outcome in a report.
/// `reason` is `Some` when it is skipped; `id` and `path` are `Some` once taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreRestore {
    pub(crate) reason: Option<&'static str>,
    pub(crate) id: Option<String>,
    pub(crate) path: Option<PathBuf>,
}

/// Everything a restore needs once every refusal has passed; owns its staged members.
pub(crate) struct Plan {
    pub(crate) snapshot_id: String,
    pub(crate) created_at: String,
    pub(crate) target: ResolvedTarget,
    pub(crate) backup_dir: PathBuf,
    pub(crate) target_exists: bool,
    pub(crate) counts_before: Counts,
    pub(crate) counts_incoming: Counts,
    pub(crate) schema: Schema,
    /// The archive's dump meta, when the archive is a Postgres dump.
    pub(crate) dump_meta: Option<clio_backup::DumpMeta>,
    /// The store schema the restore loads into, when the target is Postgres.
    pub(crate) store_schema: Option<String>,
    pub(crate) pre_restore: PreRestore,
    /// Whether the operator asked for no pre-restore snapshot. Carried into
    /// `place` because the skip decision is taken again there, after the
    /// prompt, and it has to consult the same flag.
    pub(crate) no_pre_backup: bool,
    pub(crate) dry_run: bool,
    pub(crate) key: Option<KeyPlacement>,
    /// The owner-only erasure journal the guarded restore re-checks and locks.
    pub(crate) journal_path: PathBuf,
    /// The authority the archive claims, classified at plan time.
    pub(crate) claim: ArchiveClaim,
    /// Every member's subject-reference fact, classified at plan time.
    pub(crate) members: Vec<MemberFacts>,
    pub(crate) extracted: ExtractedArchive,
}

/// What one restore reports, after planning or after placing.
pub(crate) struct Report {
    pub(crate) snapshot_id: String,
    pub(crate) target: ResolvedTarget,
    pub(crate) pre_restore: PreRestore,
    pub(crate) counts_before: Counts,
    pub(crate) counts_after: Option<Counts>,
    pub(crate) counts_incoming: Option<Counts>,
    pub(crate) schema: Schema,
    /// The store schema the restore loaded into, when the target is Postgres.
    pub(crate) store_schema: Option<String>,
    pub(crate) result: &'static str,
    pub(crate) failed_step: Option<String>,
    pub(crate) failure: Option<String>,
    pub(crate) dry_run: bool,
    /// Whether the archive carries a key store the restore places. `true` when
    /// the plan has a key placement to make.
    pub(crate) key_restored: bool,
    /// What the swap did to the key store: `None` when no key store was part of
    /// the restore (including a dry run, which places nothing), `Some(true)`
    /// when it was replaced, `Some(false)` when it was not.
    pub(crate) key_replaced: Option<bool>,
    /// Whether the target's directory was fsynced after the rename; `None`
    /// when nothing was renamed (a dry run, or a Postgres restore, which
    /// renames no file).
    pub(crate) directory_synced: Option<bool>,
}

/// Resolve, check, and stage one restore, up to the point of no return: every
/// refusal lives here, so a refused run has taken no snapshot and touched
/// nothing live. The returned plan owns the staged members until it is placed.
pub(crate) fn plan(
    env: &dyn Fn(&str) -> Option<String>,
    parsed: &ParsedArgs,
    current_dir: &Path,
    check: &dyn SpaceCheck,
) -> Result<Plan, CliError> {
    let get = |key: &str| env(key);
    let backup = resolve_backup_dir(&get, current_dir);
    let rows = match clio_backup::list_snapshot_rows(&backup.path, None) {
        Ok(rows) => rows,
        Err(error) if error.code == clio_types::ErrorCode::NotFound => Vec::new(),
        Err(error) => {
            return Err(backup_restore_exit::from_am_error(
                backup_restore_exit::RestoreFailure::BackupDirectoryUnreadable,
                &error,
            ));
        }
    };
    let id = resolve_restore_id(parsed, &rows)?;
    let archive = backup.path.join(clio_backup::archive_file_name(&id));
    let manifest = clio_backup::read_manifest(&archive).map_err(|error| {
        // A listing that named this id and a file that is gone by the manifest
        // read is the machine's condition, not a bad argument, so the code
        // comes from the table rather than from this call site.
        backup_restore_exit::from_am_error(backup_restore_exit::manifest_failure(&error), &error)
    })?;
    let mut target = resolve_restore_target(&get, parsed, "restore")?;
    if !target.is_postgres() && target.path.is_relative() {
        target.path = current_dir.join(&target.path);
        target.kms_path = target.kms_path.map(|kms| current_dir.join(kms));
        // The reports show the absolute path a restore actually reads.
        target.display = target.path.display().to_string();
    }
    refuse_backend_mismatch(&manifest, &target)?;
    let schema = compare_schema(
        manifest.schema_version.as_deref(),
        clio_store::SCHEMA_VERSION,
    )?;
    refuse_missing_keys(&manifest, parsed)?;
    // The authority gate: the archive is checked against the current journal
    // before any plan is built; the same check runs again under the lock.
    // One shared resolution: the restore CLI and the MCP erase boundary resolve
    // the same erasure journal, so the lock this restore takes is the lock an
    // erasure denial consults.
    let journal_path = JournalHold::resolve_journal_path(None, env)
        .map_err(|error| CliError::refusal(error.code.as_str(), error.message))?;
    let authority =
        classify_archive(&archive, &manifest).map_err(|refusal| classify_refusal(&refusal))?;
    clio_compliance::check_restore_at_plan(
        &journal_path,
        None,
        &authority.claim,
        &authority.members,
    )
    .map_err(authority_refusal)?;
    let postgres = manifest.store.backend == "postgres";
    let input = RestorePlanInput {
        parsed,
        manifest,
        target,
        schema,
        id,
        archive: &archive,
        backup_dir: &backup.path,
        check,
        journal_path,
        authority,
    };
    if postgres {
        plan_postgres(input)
    } else {
        crate::backup_restore_sqlite::plan_sqlite(input)
    }
}

/// Everything one plan half needs, bundled so the halves share one shape and
/// the dispatch stays a single decision.
pub(crate) struct RestorePlanInput<'a> {
    pub(crate) parsed: &'a ParsedArgs,
    pub(crate) manifest: BackupManifest,
    pub(crate) target: ResolvedTarget,
    pub(crate) schema: Schema,
    pub(crate) id: String,
    pub(crate) archive: &'a Path,
    pub(crate) backup_dir: &'a Path,
    pub(crate) check: &'a dyn SpaceCheck,
    pub(crate) journal_path: PathBuf,
    pub(crate) authority: ClassifiedArchive,
}

/// The report for a dry run: the plan, with nothing placed.
pub(crate) fn planned(plan: Plan) -> Report {
    Report {
        snapshot_id: plan.snapshot_id,
        target: plan.target,
        pre_restore: plan.pre_restore,
        counts_before: plan.counts_before,
        counts_after: None,
        counts_incoming: Some(plan.counts_incoming),
        schema: plan.schema,
        store_schema: plan.store_schema,
        result: RESULT_PLANNED,
        failed_step: None,
        failure: None,
        dry_run: true,
        key_restored: plan.key.is_some(),
        key_replaced: None,
        directory_synced: None,
    }
}

/// Place a planned restore: snapshot the current store first, then place.
pub(crate) fn place(plan: Plan) -> Result<Report, CliError> {
    // The durable event store beside the journal is outside every replaced
    // path. The journal hold and the pending-action reconcile both happen
    // inside the placement guard, under one lock, so the lock proves no peer
    // restore is mid placement.
    let events = open_restore_events(&plan.journal_path)?;
    let now = clio_mcp::runtime::now_utc();
    let placement = RestorePlacement {
        events: &*events,
        now: &now,
    };
    if plan.target.is_postgres() {
        place_postgres(plan, placement)
    } else {
        crate::backup_restore_sqlite::place_sqlite(plan, placement)
    }
}

/// A classification refusal as a clear restore refusal, keeping its own code.
pub(crate) fn classify_refusal(refusal: &ClassifyRefusal) -> CliError {
    let code = refusal.code();
    let error = CliError::refusal(code.as_str(), refusal.to_string());
    match code {
        clio_types::ErrorCode::ArchiveCorrupt | clio_types::ErrorCode::Internal => {
            error.with_hint(HINT_ARCHIVE_DAMAGED)
        }
        _ => error,
    }
}

/// An authority refusal as a clear restore refusal, keeping its own code.
fn authority_refusal(refusal: AuthorityRefusal) -> CliError {
    CliError::refusal(refusal.code().as_str(), refusal.to_string())
}

/// The pre-restore outcome a placement starts from, from a decision taken
/// against a fresh measurement.
///
/// The skip decision was taken from a measurement made before the operator was
/// asked, and the confirmation prompt waits on a human for as long as they
/// take. So the placement measures the target again and decides again from that
/// measurement: a store that appeared, filled, or emptied while the prompt was
/// up must not inherit a decision from a stale measurement, because skipping
/// the snapshot on that basis replaces rows with no way back.
pub(crate) fn pre_restore_from(plan: &Plan, decided: PreRestore) -> Result<PreRestore, CliError> {
    if decided.reason.is_some() {
        Ok(decided)
    } else {
        take_pre_restore(plan)
    }
}

/// Take the pre-restore snapshot, or fail the run with the create error.
pub(crate) fn take_pre_restore(plan: &Plan) -> Result<PreRestore, CliError> {
    let label = format!("pre-restore snapshot of {}", plan.snapshot_id);
    let authority = crate::backup_create_cli::current_authority(&plan.journal_path);
    let created = take_snapshot(
        &plan.target,
        &plan.backup_dir,
        Some(&label),
        None,
        authority,
    )
    .map_err(|error| {
        backup_restore_exit::from_am_error(backup_restore_exit::snapshot_failure(&error), &error)
    })?;
    Ok(PreRestore {
        reason: None,
        id: Some(created.archive.id),
        path: Some(created.archive.path),
    })
}

/// Refuse an archive whose backend is not the target's, naming both.
fn refuse_backend_mismatch(
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
fn refuse_missing_keys(manifest: &BackupManifest, parsed: &ParsedArgs) -> Result<(), CliError> {
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

/// The pre-restore snapshot decision: take it, or say why not.
///
/// `no_pre_backup` is the operator's own flag, and `target_exists` and
/// `target_empty` are the backend-neutral facts both measurements produce, so
/// the decision cannot differ between backends.
pub(crate) fn pre_restore_decision(
    no_pre_backup: bool,
    target_exists: bool,
    target_empty: bool,
) -> PreRestore {
    let reason = if no_pre_backup {
        Some(SKIP_NO_PRE_BACKUP)
    } else if !target_exists {
        Some(SKIP_TARGET_MISSING)
    } else if target_empty {
        Some(SKIP_TARGET_EMPTY)
    } else {
        None
    };
    PreRestore {
        reason,
        id: None,
        path: None,
    }
}

#[cfg(test)]
#[path = "backup_restore_flow_tests.rs"]
mod tests;

/// The plan's unreadable-backup-directory and key-size refusals.
#[cfg(test)]
#[path = "backup_restore_flow_plan_tests.rs"]
mod plan_refusal_tests;
