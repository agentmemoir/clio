// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the restore's condition-to-exit-code table: that every
//! condition has exactly one row, that each row maps to the code the command
//! catalog documents, and that the `AmError` classifiers route one condition to
//! one code whichever half of the run noticed it.
//!
//! ## Owns
//! - `each_condition`, the wildcard-free enumeration of every `RestoreFailure`
//!   variant that makes the table's completeness a compile-time fact
//! - The per-row code assertions
//! - The plan-time and place-time agreement for the space conditions
//! - The classifiers' routing by error code
//!
//! ## Does not own
//! - The table's data (see `backup_restore_exit`)
//! - The restore sequence that raises each condition (see `backup_restore_flow`
//!   and the two backend halves)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Only the table's mapping
//! is pinned here; a condition that is never raised is a dead row, not a wrong
//! one, and the restore suites cover the raising.
//!
//! Keep this module focused on the table's rows and classifiers.

use super::*;
use clio_types::AmError;

/// The message `df` failing to run produces: the operator is shown the error's
/// own text, never a paraphrase.
const DF_FAILED: &str =
    "cannot check free space in /data: `df` failed: No such file or directory (os error 2)";

/// Visit every `RestoreFailure` variant, in declaration order.
///
/// The `match` inside the walk has one arm per variant and no wildcard arm, so
/// a new variant fails to compile here instead of quietly having no row in the
/// exit-code table and panicking at run time on a real restore failure. A
/// `match` runs one arm, so each arm hands the next variant to the same walk:
/// the one statement is both the proof that the list is complete and the list
/// itself, with no second copy of the enum to fall out of step.
fn each_condition(mut visit: impl FnMut(RestoreFailure)) {
    fn walk(next: RestoreFailure, visit: &mut impl FnMut(RestoreFailure)) {
        use RestoreFailure::*;
        visit(next);
        match next {
            RestoredOrPlanned => walk(RestoredNotDurable, visit),
            RestoredNotDurable => walk(PartialOrCountMismatch, visit),
            PartialOrCountMismatch => walk(BadArguments, visit),
            BadArguments => walk(ArchiveDamaged, visit),
            ArchiveDamaged => walk(BackendMismatch, visit),
            BackendMismatch => walk(SchemaTooNew, visit),
            SchemaTooNew => walk(ArchiveWithoutKeys, visit),
            ArchiveWithoutKeys => walk(KeyNowhereToLive, visit),
            KeyNowhereToLive => walk(TableSetMismatch, visit),
            TableSetMismatch => walk(TargetUnmeasurable, visit),
            TargetUnmeasurable => walk(TargetNotWritable, visit),
            TargetNotWritable => walk(PreRestoreSnapshotRefused, visit),
            PreRestoreSnapshotRefused => walk(StoreSwapRefused, visit),
            StoreSwapRefused => walk(PostgresTargetRefused, visit),
            PostgresTargetRefused => walk(PostgresLoadRefused, visit),
            PostgresLoadRefused => walk(CurrentDirectoryUnavailable, visit),
            CurrentDirectoryUnavailable => walk(BackupDirectoryUnreadable, visit),
            BackupDirectoryUnreadable => walk(ArchiveVanished, visit),
            ArchiveVanished => walk(ArchiveUnreadable, visit),
            ArchiveUnreadable => walk(OutOfSpace, visit),
            OutOfSpace => walk(MemberNotStaged, visit),
            MemberNotStaged => walk(PreRestoreSnapshotFailed, visit),
            PreRestoreSnapshotFailed => walk(StoreSwapFailed, visit),
            StoreSwapFailed => walk(PostgresLoadFailed, visit),
            PostgresLoadFailed => {}
        }
    }
    walk(RestoreFailure::RestoredOrPlanned, &mut visit);
}

/// The same list, collected: declaration order is the order the table is
/// compared in, so the comparison never depends on the table's own order.
fn all_conditions() -> Vec<RestoreFailure> {
    let mut conditions = Vec::new();
    each_condition(|condition| {
        conditions.push(condition);
    });
    conditions
}

/// A condition's place in the declaration order, `usize::MAX` if it has none.
/// `RestoreFailure` is `Copy` and one byte wide, so it is taken by value.
fn declaration_rank(conditions: &[RestoreFailure], condition: RestoreFailure) -> usize {
    conditions
        .iter()
        .position(|known| *known == condition)
        .unwrap_or(usize::MAX)
}

/// The process exit code a built error must report for a table code.
fn exit(code: u8) -> std::process::ExitCode {
    std::process::ExitCode::from(code)
}

/// One exit code per row, asserted one row at a time so a failure names the row
/// that moved rather than printing the whole table.
#[test]
fn every_condition_maps_to_the_documented_code() {
    let expected = [
        (RestoreFailure::RestoredOrPlanned, EXIT_OK),
        (RestoreFailure::RestoredNotDurable, EXIT_PARTIAL),
        (RestoreFailure::PartialOrCountMismatch, EXIT_PARTIAL),
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
    for (condition, code) in expected {
        assert_eq!(
            exit_code_for(condition),
            code,
            "{condition:?} maps to the wrong exit code"
        );
    }
}

#[test]
fn every_condition_has_exactly_one_row_and_no_row_is_extra() {
    let conditions = all_conditions();

    // Every condition has a row: `exit_code_for` panics when it does not, so
    // reaching the end of this loop is the proof, over the compiler-checked list
    // rather than a hand-kept copy of the enum.
    for condition in &conditions {
        let _ = exit_code_for(*condition);
    }

    assert_eq!(
        RESTORE_EXIT_CODES.len(),
        conditions.len(),
        "one row per condition, with no duplicate and no row that names nothing"
    );
    let mut rows: Vec<RestoreFailure> = RESTORE_EXIT_CODES.iter().map(|(row, _)| *row).collect();
    rows.sort_by_key(|condition| declaration_rank(&conditions, *condition));
    assert_eq!(
        rows, conditions,
        "the table and the enumerated variants must be the same list, once each"
    );
}

#[test]
fn every_row_is_one_of_the_four_documented_codes() {
    for (condition, code) in RESTORE_EXIT_CODES {
        assert!(
            matches!(
                *code,
                EXIT_OK | EXIT_OPERATIONAL | EXIT_USAGE | EXIT_PARTIAL
            ),
            "{condition:?} maps to {code}, which is outside the documented contract"
        );
    }
}

#[test]
fn a_constructed_failure_carries_the_tables_code_not_its_constructors() {
    let error = super::error(RestoreFailure::OutOfSpace, "QuotaExceeded", "no room");

    assert_eq!(
        error.exit_code(),
        std::process::ExitCode::from(EXIT_OPERATIONAL)
    );
    assert_eq!(
        error.code(),
        "QuotaExceeded",
        "the machine code is the caller's"
    );
}

#[test]
fn a_refusal_row_keeps_the_usage_code() {
    let error = super::error(
        RestoreFailure::ArchiveWithoutKeys,
        "missing_keys",
        "no keys",
    );

    assert_eq!(error.exit_code(), std::process::ExitCode::from(EXIT_USAGE));
}

#[test]
fn an_unmeasurable_target_names_lock_contention_not_only_permissions() {
    let error = super::error(
        RestoreFailure::TargetUnmeasurable,
        "invalid_argument",
        "cannot measure /data/clio.db",
    );

    let hint = error
        .hint()
        .expect("the target hint is what the operator acts on");
    assert!(
        hint.contains("another `clio`"),
        "a running clio holding the store is the usual cause: {hint}"
    );
    assert!(
        hint.contains("permissions"),
        "the permissions check is still one of the causes: {hint}"
    );
}

#[test]
fn a_damaged_archive_gets_the_damaged_archive_hint() {
    let error = super::error(
        RestoreFailure::ArchiveDamaged,
        "ArchiveCorrupt",
        "truncated",
    );

    assert_eq!(error.hint(), Some(HINT_ARCHIVE_DAMAGED));
}

#[test]
fn a_staging_failure_is_never_told_the_archive_is_damaged() {
    let error = super::error(
        RestoreFailure::MemberNotStaged,
        "QuotaExceeded",
        "disk full",
    );

    assert_eq!(error.hint(), Some(HINT_STAGING_FAILED));
    assert_ne!(error.hint(), Some(HINT_ARCHIVE_DAMAGED));
}

#[test]
fn an_out_of_space_footprint_is_operational_at_plan_time() {
    let error = AmError::new(ErrorCode::QuotaExceeded, "no room");

    assert_eq!(space_failure(&error), RestoreFailure::OutOfSpace);
    assert_eq!(
        exit_code_for(space_failure(&error)),
        EXIT_OPERATIONAL,
        "a directory with no room is the machine's condition, not a refusal"
    );
}

#[test]
fn the_same_out_of_space_error_maps_to_one_code_whichever_half_raised_it() {
    let error = AmError::new(ErrorCode::QuotaExceeded, "no room");

    let at_plan = super::from_am_error(space_failure(&error), &error);
    let at_place = super::from_am_error(snapshot_failure(&error), &error);

    assert_eq!(at_plan.exit_code(), at_place.exit_code());
}

#[test]
fn a_directory_that_will_not_take_writes_is_a_refusal_at_both_halves() {
    let at_plan = AmError::new(ErrorCode::InvalidArgument, "not writable");
    let at_place = AmError::new(ErrorCode::InvalidArgument, "not writable");

    assert_eq!(space_failure(&at_plan), RestoreFailure::TargetNotWritable);
    assert_eq!(exit_code_for(space_failure(&at_plan)), EXIT_USAGE);
    assert_eq!(
        super::from_am_error(snapshot_failure(&at_place), &at_place).exit_code(),
        std::process::ExitCode::from(EXIT_USAGE),
        "take_pre_restore's InvalidArgument is the same class as a refusal in the plan half"
    );
}

#[test]
fn a_missing_archive_is_operational_not_a_usage_error() {
    let error = AmError::new(ErrorCode::NotFound, "no such file");

    assert_eq!(manifest_failure(&error), RestoreFailure::ArchiveVanished);
    assert_eq!(exit_code_for(manifest_failure(&error)), EXIT_OPERATIONAL);
}

#[test]
fn a_damaged_manifest_is_still_a_refusal() {
    let error = AmError::new(ErrorCode::ArchiveCorrupt, "bad json");

    assert_eq!(manifest_failure(&error), RestoreFailure::ArchiveDamaged);
    assert_eq!(exit_code_for(manifest_failure(&error)), EXIT_USAGE);
}

#[test]
fn an_extraction_that_could_not_be_staged_is_not_archive_damage() {
    let full = AmError::new(ErrorCode::QuotaExceeded, "no space left");
    let other = AmError::new(ErrorCode::Internal, "permission denied");

    assert_eq!(extraction_failure(&full), RestoreFailure::OutOfSpace);
    assert_eq!(
        extraction_failure(&other),
        RestoreFailure::MemberNotStaged,
        "a staging failure is the machine's condition"
    );
}

#[test]
fn a_swap_refusal_and_a_swap_failure_map_apart() {
    let refused = AmError::new(ErrorCode::InvalidArgument, "an open handle");
    let failed = AmError::new(ErrorCode::Internal, "rename failed");

    assert_eq!(swap_failure(&refused), RestoreFailure::StoreSwapRefused);
    assert_eq!(exit_code_for(swap_failure(&refused)), EXIT_USAGE);
    assert_eq!(swap_failure(&failed), RestoreFailure::StoreSwapFailed);
    assert_eq!(exit_code_for(swap_failure(&failed)), EXIT_OPERATIONAL);
}

#[test]
fn a_postgres_load_the_role_may_not_run_is_a_refusal() {
    let refused = AmError::new(ErrorCode::Forbidden, "does not own the tables");
    let failed = AmError::new(ErrorCode::Internal, "connection lost");

    assert_eq!(
        postgres_load_failure(&refused),
        RestoreFailure::PostgresLoadRefused
    );
    assert_eq!(exit_code_for(postgres_load_failure(&refused)), EXIT_USAGE);
    assert_eq!(
        postgres_load_failure(&failed),
        RestoreFailure::PostgresLoadFailed
    );
    assert_eq!(
        exit_code_for(postgres_load_failure(&failed)),
        EXIT_OPERATIONAL
    );
}

#[test]
fn an_in_process_error_keeps_its_own_code_and_message() {
    let error = AmError::new(ErrorCode::ArchiveCorrupt, "member 3: digest differs");

    let mapped = super::from_am_error(manifest_failure(&error), &error);

    assert_eq!(mapped.code(), "archive_corrupt");
    assert_eq!(mapped.message(), "member 3: digest differs");
}

#[test]
fn a_condition_with_no_row_would_fail_to_compile_rather_than_panic_at_run_time() {
    // The list this loop walks has no wildcard arm, so a condition that could
    // reach `exit_code_for` without a row is a compile error in this file, not a
    // panic on a real restore failure. What is left is to prove the lookup
    // answers for every variant.
    for condition in all_conditions() {
        let code = exit_code_for(condition);
        assert!(
            matches!(code, EXIT_OK | EXIT_OPERATIONAL | EXIT_USAGE | EXIT_PARTIAL),
            "{condition:?} maps to {code}, which is outside the documented contract"
        );
    }
}

#[test]
fn a_space_check_that_cannot_answer_is_operational_not_a_refusal() {
    // The shape `run_available_bytes` uses for every way `df` fails to answer.
    let error = AmError::new(ErrorCode::Internal, DF_FAILED);

    assert_eq!(space_failure(&error), RestoreFailure::OutOfSpace);
    assert_eq!(
        exit_code_for(space_failure(&error)),
        EXIT_OPERATIONAL,
        "a machine that could not answer is not a request the operator got wrong"
    );

    // The last part is what keeps the operator from being told "no room": the
    // condition name is internal, so the error's own code and message are all
    // the operator is shown.
    let built = super::from_am_error(space_failure(&error), &error);
    assert_eq!(built.code(), "internal", "the error's own code");
    assert_eq!(
        built.message(),
        DF_FAILED,
        "the real message from df is what reaches the operator"
    );
    assert_eq!(built.exit_code(), exit(EXIT_OPERATIONAL));
}

#[test]
fn an_archive_over_a_reader_cap_is_a_refusal_not_a_machine_out_of_space() {
    // The shape the reader raises for a member over one of its caps. The cap is
    // a property of the file, so no amount of free space lets this build read
    // it, and a hint blaming staging sends the operator to the wrong disk.
    let cap = AmError::new(
        ErrorCode::ArchiveCorrupt,
        "member 4: 4194304 bytes is over the 1048576 byte per-member cap",
    );

    assert_eq!(extraction_failure(&cap), RestoreFailure::ArchiveDamaged);
    assert_eq!(
        exit_code_for(extraction_failure(&cap)),
        EXIT_USAGE,
        "an archive this build refuses to read is a refusal, not the machine's condition"
    );

    let built = super::from_am_error(extraction_failure(&cap), &cap);
    assert_eq!(built.hint(), Some(HINT_ARCHIVE_DAMAGED));
    assert_ne!(
        built.hint(),
        Some(HINT_STAGING_FAILED),
        "blaming staging would send the operator to free space that is not the problem"
    );
}

#[test]
fn a_full_directory_while_staging_is_still_out_of_space() {
    // The other direction: a genuinely full disk is still exit 1, so the cap
    // change above cannot have swallowed it. `OutOfSpace` carries no hint of
    // its own; `HINT_STAGING_FAILED` belongs to `MemberNotStaged`, pinned by
    // the staging-failure test above.
    let full = AmError::new(ErrorCode::QuotaExceeded, "No space left on device");

    assert_eq!(extraction_failure(&full), RestoreFailure::OutOfSpace);
    assert_eq!(exit_code_for(extraction_failure(&full)), EXIT_OPERATIONAL);
    let built = super::from_am_error(extraction_failure(&full), &full);
    assert_eq!(built.exit_code(), exit(EXIT_OPERATIONAL));
    assert_ne!(
        built.hint(),
        Some(HINT_ARCHIVE_DAMAGED),
        "a full disk is not a damaged archive, and freeing space cannot raise a cap"
    );
}
