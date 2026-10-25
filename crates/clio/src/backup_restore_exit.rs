// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one thing: which process exit code each
//! `clio backup restore` condition maps to, and which operator hint goes with
//! it. It is the single source of that mapping for the command.
//!
//! ## Owns
//! - `RestoreFailure`, every condition a restore can fail or degrade on
//! - `RESTORE_EXIT_CODES`, the condition-to-exit-code table, and `exit_code_for`
//! - The `AmError`-code classifiers that turn a subsystem error into a condition
//!   before the table is consulted
//!
//! ## Does not own
//! - The restore sequence and where each failure is raised (see
//!   `backup_restore_flow` and the two backend halves)
//! - The exit-code constants themselves (see `cli_error`)
//! - The text and JSON views of a report (see `backup_restore_report`)
//!
//! ## Boundary
//!
//! Changes to which exit code a restore condition maps to should normally
//! require changes here, and nowhere else. Changes to when a condition is
//! detected should not require changes here.
//!
//! Keep this module focused on the restore's condition-to-code table.

use crate::backup_restore_flow::{
    HINT_ARCHIVE_DAMAGED, HINT_STAGING_FAILED, HINT_TARGET_UNMEASURABLE,
};
use crate::cli_error::{
    CliError, EXIT_OK, EXIT_OPERATIONAL, EXIT_PARTIAL, EXIT_TEMPORARY, EXIT_USAGE,
};
use clio_types::{AmError, ErrorCode};

/// One condition a `clio backup restore` run can fail or degrade on.
///
/// A call site names the condition it hit and the table decides the code, so
/// the same condition cannot map to two codes because it was detected at plan
/// time in one path and at place time in another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreFailure {
    // Success and degraded outcomes, which are results rather than failures but
    // still decide the process exit code a script branches on.
    /// A restore that completed, or a dry run that changed nothing.
    RestoredOrPlanned,
    /// A restore that replaced the database but not everything it promised:
    /// a failed key swap, or a post-commit count check that did not match.
    /// A rename that could not be fsynced also lands here: the store was
    /// replaced but the report is degraded and must not exit 0.
    RestoredNotDurable,
    /// The database was replaced but the run did not fully complete.
    PartialOrCountMismatch,
    /// Every user-data row was loaded, and only a derived table the store can
    /// rebuild holds a different number of rows than the archive recorded. The
    /// restore did its job, so this is not a failure; it takes the temporary
    /// code because the run could not report a clean success, and retrying it
    /// reloads the same archive and reproduces the same count, so the retry that
    /// clears a lock conflict clears nothing here.
    DerivedTableDiffers,

    // Refusals: the input was understood and deliberately not acted on, so
    // nothing live was touched. Exit 2.
    /// A flag, selector, typed id, or output mode the command does not accept.
    BadArguments,
    /// A member the archive declares is missing, truncated, or does not match
    /// the manifest that declares it.
    ArchiveDamaged,
    /// The archive records a different backend than the target store.
    BackendMismatch,
    /// The archive was written by a newer store schema than this build opens.
    SchemaTooNew,
    /// The archive carries no key store and the operator did not accept that.
    ArchiveWithoutKeys,
    /// The archive carries a key member but the target has no path to place it.
    KeyNowhereToLive,
    /// The archive's table set does not match the Postgres target's.
    TableSetMismatch,
    /// The target store could not be inspected, so nothing was measured and
    /// nothing was touched.
    TargetUnmeasurable,
    /// A footprint's directory does not accept writes.
    TargetNotWritable,
    /// The pre-restore snapshot was refused on its own terms.
    PreRestoreSnapshotRefused,
    /// The store swap refused before replacing anything.
    StoreSwapRefused,
    /// The Postgres target could not be reached or read.
    PostgresTargetRefused,
    /// The Postgres load was refused before truncating the target.
    PostgresLoadRefused,

    // Operational failures: the machine could not do it now. Exit 1.
    /// The process could not determine the directory it was launched in.
    CurrentDirectoryUnavailable,
    /// The backup directory could not be listed.
    BackupDirectoryUnreadable,
    /// The archive a listing named was gone by the time it was read.
    ArchiveVanished,
    /// The archive could not be read for a reason that is not its contents.
    ArchiveUnreadable,
    /// A footprint's directory has no room for the bytes it must hold, or its
    /// free space could not be measured at all.
    ///
    /// The second case is why this row is right. The condition name is internal
    /// and is never printed to the operator: the operator sees the error's own
    /// `internal` machine code and the real message from `df`, so nothing tells
    /// them "no room" when the machine could not answer. Exit 1 is the honest
    /// class for a machine that could not answer, where a refusal would claim
    /// the request itself was the wrong thing to make.
    OutOfSpace,
    /// A declared member could not be written to the staging directory.
    MemberNotStaged,
    /// The pre-restore snapshot could not be taken.
    PreRestoreSnapshotFailed,
    /// The store swap failed before the rename.
    StoreSwapFailed,
    /// The Postgres load failed; its own transaction rolled the target back.
    PostgresLoadFailed,
}

/// The table: the one place a restore condition becomes a process exit code.
///
/// Data rather than a chain of `if`s, so the whole contract is readable in one
/// place and a test can assert one row at a time. The codes are the documented
/// `0` ok / `1` operational / `2` usage / `3` degraded / `75` not-a-clean-success
/// contract, and `75` carries the one meaning `EXIT_TEMPORARY_MEANING` states.
/// `DerivedTableDiffers` is the one row on the reused `75` here: the restore
/// loaded every user-data row, lost nothing, and invalidated nothing, and the
/// difference it reports is not something a retry of the restore clears.
const RESTORE_EXIT_CODES: &[(RestoreFailure, u8)] = &[
    (RestoreFailure::RestoredOrPlanned, EXIT_OK),
    (RestoreFailure::RestoredNotDurable, EXIT_PARTIAL),
    (RestoreFailure::PartialOrCountMismatch, EXIT_PARTIAL),
    (RestoreFailure::DerivedTableDiffers, EXIT_TEMPORARY),
    (RestoreFailure::BadArguments, EXIT_USAGE),
    (RestoreFailure::ArchiveDamaged, EXIT_USAGE),
    (RestoreFailure::BackendMismatch, EXIT_USAGE),
    (RestoreFailure::SchemaTooNew, EXIT_USAGE),
    (RestoreFailure::ArchiveWithoutKeys, EXIT_USAGE),
    (RestoreFailure::KeyNowhereToLive, EXIT_USAGE),
    (RestoreFailure::TableSetMismatch, EXIT_USAGE),
    (RestoreFailure::TargetUnmeasurable, EXIT_USAGE),
    (RestoreFailure::TargetNotWritable, EXIT_USAGE),
    (RestoreFailure::PreRestoreSnapshotRefused, EXIT_USAGE),
    (RestoreFailure::StoreSwapRefused, EXIT_USAGE),
    (RestoreFailure::PostgresTargetRefused, EXIT_USAGE),
    (RestoreFailure::PostgresLoadRefused, EXIT_USAGE),
    (
        RestoreFailure::CurrentDirectoryUnavailable,
        EXIT_OPERATIONAL,
    ),
    (RestoreFailure::BackupDirectoryUnreadable, EXIT_OPERATIONAL),
    (RestoreFailure::ArchiveVanished, EXIT_OPERATIONAL),
    (RestoreFailure::ArchiveUnreadable, EXIT_OPERATIONAL),
    (RestoreFailure::OutOfSpace, EXIT_OPERATIONAL),
    (RestoreFailure::MemberNotStaged, EXIT_OPERATIONAL),
    (RestoreFailure::PreRestoreSnapshotFailed, EXIT_OPERATIONAL),
    (RestoreFailure::StoreSwapFailed, EXIT_OPERATIONAL),
    (RestoreFailure::PostgresLoadFailed, EXIT_OPERATIONAL),
];

/// The exit code one restore condition maps to.
///
/// # Panics
/// A condition with no row in the table. Every variant of `RestoreFailure` is
/// enumerated without a wildcard by `each_condition` in this module's test
/// suite, so a new variant fails to compile there rather than reaching this
/// lookup; `every_condition_has_exactly_one_row_and_no_row_is_extra` then calls
/// this function once per enumerated variant, which is what proves every
/// variant has a row here.
#[must_use]
pub(crate) fn exit_code_for(condition: RestoreFailure) -> u8 {
    RESTORE_EXIT_CODES
        .iter()
        .find(|(row, _)| *row == condition)
        .map_or_else(
            || panic!("every restore condition has a row in the exit-code table"),
            |(_, code)| *code,
        )
}

/// The failure a caller reports, with the table's exit code and the caller's
/// own machine code and message.
pub(crate) fn error(condition: RestoreFailure, code: &str, message: impl Into<String>) -> CliError {
    let exit = exit_code_for(condition);
    let built = if exit == EXIT_USAGE {
        CliError::refusal(code, message)
    } else {
        CliError::operational(code, message)
    };
    match hint_for(condition) {
        Some(hint) => built.with_hint(hint).with_exit(exit),
        None => built.with_exit(exit),
    }
}

/// The failure an in-process error carries, with its own machine code and
/// message and the table's exit code.
pub(crate) fn from_am_error(condition: RestoreFailure, error: &AmError) -> CliError {
    self::error(condition, error.code.as_str(), error.message.clone())
}

/// The operator hint for a condition, or `None` when the code and the message
/// are already enough to act on.
///
/// The hint is chosen from the condition rather than attached at the call site,
/// because the defect this table fixes is a hint that named the wrong cause: a
/// staging directory that is full must not be told the archive is damaged.
fn hint_for(condition: RestoreFailure) -> Option<&'static str> {
    match condition {
        RestoreFailure::ArchiveDamaged => Some(HINT_ARCHIVE_DAMAGED),
        RestoreFailure::MemberNotStaged => Some(HINT_STAGING_FAILED),
        RestoreFailure::TargetUnmeasurable => Some(HINT_TARGET_UNMEASURABLE),
        _ => None,
    }
}

/// The condition a space or writability failure maps to, from the error's own
/// code.
///
/// The same rule runs at plan time and at place time, so a directory with no
/// room is the same code whichever half of the run noticed. A directory that
/// will not accept writes is a refusal instead: nothing was attempted.
///
/// The codes a space check can raise are named one by one. `Internal` is the
/// one `df` uses for every way it cannot answer — no `df` at the four fixed
/// paths, a non-zero exit, or a report with no readable available-space column —
/// so it is a machine that could not check, which is operational, not a
/// refusal. Mapping it to the refusal made a host with no `df` refuse every
/// restore at exit 2 with a condition name that named the wrong cause.
#[must_use]
pub(crate) fn space_failure(error: &AmError) -> RestoreFailure {
    match error.code {
        // The directory does not accept writes: the real refusal, and the only
        // one, because nothing was attempted against it.
        ErrorCode::InvalidArgument => RestoreFailure::TargetNotWritable,
        // Everything else is the machine: fewer bytes free than the footprint
        // needs, the free space not measurable at all (`Internal` is the code
        // `df` uses for every way it cannot answer), or a code the space check
        // never raises. A code that is not the filesystem refusing a write is
        // not evidence of a refusal, so it stays on the operational side
        // instead of being reported to a script as "you asked for the wrong
        // thing". The wildcard is here because `ErrorCode` is an open set by
        // design: naming three arms and refusing to compile whenever an
        // unrelated code is added would make this the one place in the crate
        // that breaks on a change nothing else cares about.
        _ => RestoreFailure::OutOfSpace,
    }
}

/// The condition a member-extraction failure maps to, from the error's own code.
#[must_use]
pub(crate) fn extraction_failure(error: &AmError) -> RestoreFailure {
    if error.code == ErrorCode::ArchiveCorrupt {
        RestoreFailure::ArchiveDamaged
    } else if error.code == ErrorCode::QuotaExceeded {
        RestoreFailure::OutOfSpace
    } else if error.code == ErrorCode::NotFound {
        RestoreFailure::ArchiveVanished
    } else {
        RestoreFailure::MemberNotStaged
    }
}

/// The condition a manifest read maps to, from the error's own code.
///
/// A missing archive is not a usage error: the listing named an id, the file
/// was there, and by the time it was read it was not. A script reading exit 1
/// would conclude it had asked for the wrong thing.
#[must_use]
pub(crate) fn manifest_failure(error: &AmError) -> RestoreFailure {
    match error.code {
        ErrorCode::NotFound => RestoreFailure::ArchiveVanished,
        ErrorCode::ArchiveCorrupt => RestoreFailure::ArchiveDamaged,
        _ => RestoreFailure::ArchiveUnreadable,
    }
}

/// The condition a store swap maps to, from the error's own code. The swap's
/// refusals all happen before it replaces anything.
#[must_use]
pub(crate) fn swap_failure(error: &AmError) -> RestoreFailure {
    if matches!(
        error.code,
        ErrorCode::InvalidArgument | ErrorCode::ArchiveCorrupt
    ) {
        RestoreFailure::StoreSwapRefused
    } else {
        RestoreFailure::StoreSwapFailed
    }
}

/// The condition the pre-restore snapshot maps to, from the error's own code.
///
/// `InvalidArgument` is the same class as every other refusal in the plan half,
/// so it exits 2 here too rather than the 1 the bare mapping gave it.
#[must_use]
pub(crate) fn snapshot_failure(error: &AmError) -> RestoreFailure {
    if error.code == ErrorCode::InvalidArgument {
        RestoreFailure::PreRestoreSnapshotRefused
    } else {
        RestoreFailure::PreRestoreSnapshotFailed
    }
}

/// The condition a Postgres load maps to, from the error's own code. The store
/// half's `Forbidden` is its deliberate refusal to truncate a target the
/// connected role does not own, raised before anything is replaced.
#[must_use]
pub(crate) fn postgres_load_failure(error: &AmError) -> RestoreFailure {
    if error.code == ErrorCode::Forbidden {
        RestoreFailure::PostgresLoadRefused
    } else {
        RestoreFailure::PostgresLoadFailed
    }
}

#[cfg(test)]
#[path = "backup_restore_exit_tests.rs"]
mod tests;
