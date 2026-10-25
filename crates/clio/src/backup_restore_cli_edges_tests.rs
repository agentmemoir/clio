// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the restore surface's early refusals and view edges that
//! do not fit the main restore suite: the process-seam failures (a failing
//! current directory), the flag and output-mode refusals, the durability
//! warning's rendering in both views, and the confirmation prompt's
//! conditional clauses.
//!
//! ## Owns
//! - The early-refusal assertions
//! - The durability warning in text and in the JSON payload
//! - Every branch of the confirmation prompt's conditional clauses
//!
//! ## Does not own
//! - The restore sequence and its refusals (see `backup_restore_flow`)
//! - The condition-to-exit-code table (see `backup_restore_exit`)
//! - The golden-output and security cases (see the sibling suites)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{Scratch, run_backup, run_json_in};
use crate::backup_restore_flow::{
    Counts, FAILED_STEP_COUNTS, PreRestore, RESULT_COUNT_MISMATCH, RESULT_DERIVED_DIFFERENT,
    RESULT_RESTORED, Report, SKIP_NO_PRE_BACKUP, SKIP_TARGET_EMPTY, SKIP_TARGET_MISSING,
};
use crate::backup_restore_schema::{ACTION_NONE, Schema};
use crate::cli_error::{EXIT_PARTIAL, EXIT_TEMPORARY};
use serde_json::json;

#[test]
fn restore_with_a_failed_current_directory_is_an_internal_error() {
    let error = CliError::operational("internal", "cannot determine the current directory: boom");

    let (code, _, err) = run_backup(&["restore"], &[], Err(error), false);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("current directory"), "{err}");
}

#[test]
fn restore_refuses_an_unknown_flag_before_anything_else() {
    let scratch = Scratch::new("restore-bad-flag");

    let (code, _, err) = run_json_in(&scratch, &["restore", "--nonsense"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("unknown flag"), "{err}");
}

#[test]
fn restore_refuses_an_output_mode_it_does_not_know() {
    let scratch = Scratch::new("restore-bad-output");
    let cwd = Ok(scratch.path().to_path_buf());

    let (code, _, err) = run_backup(&["restore", "--output", "nonsense"], &[], cwd, false);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--output must be"), "{err}");
}

#[test]
fn a_report_whose_directory_fsync_failed_says_the_rename_is_not_durable() {
    let report = not_durable_report();

    let text = render(&report);

    assert!(text.contains("could not be fsynced"), "{text}");
    assert!(text.contains("not durable"), "{text}");
    assert!(text.contains("result: restored"), "{text}");
}

/// A rename that could not be fsynced is a completed restore whose result is
/// flagged as not durable. `ok` must not claim otherwise: a script that reads
/// only `ok` would record the run as clean.
#[test]
fn a_non_durable_rename_is_never_reported_as_ok_in_json() {
    let payload = payload(&not_durable_report());

    assert_eq!(payload["ok"], json!(false), "{payload}");
    assert_eq!(payload["result"], json!(RESULT_RESTORED), "{payload}");
}

/// The warning gets its own field, so a script can tell this case from both a
/// clean restore and a partial one without parsing the text.
#[test]
fn a_non_durable_rename_carries_its_own_field() {
    let payload = payload(&not_durable_report());

    assert!(
        payload["durability_warning"]
            .as_str()
            .is_some_and(|text| text.contains("not durable")),
        "{payload}"
    );
}

/// A non-durable rename is a degraded restore, so it uses exit 3 with `ok:false`.
#[test]
fn a_non_durable_rename_uses_the_degraded_exit_code() {
    let code = exit_code(&not_durable_report());

    assert_eq!(
        code,
        backup_restore_exit::exit_code_for(RestoreFailure::RestoredNotDurable)
    );
    assert_eq!(code, 3, "a degraded result must not exit 0");
}

/// A clean restore has no durability warning to report, so the field is absent
/// rather than null: its presence means something.
#[test]
fn a_durable_restore_carries_no_durability_warning() {
    let mut report = not_durable_report();
    report.directory_synced = Some(true);

    let payload = payload(&report);

    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert!(payload.get("durability_warning").is_none(), "{payload}");
}

/// The prompt is read at the moment of decision, so every clause in it is
/// conditional on this run. Under `--no-pre-backup` there is no snapshot, and
/// calling it the way back would be false.
#[test]
fn a_prompt_with_no_pre_restore_snapshot_does_not_promise_a_way_back() {
    let prompt = prompt_text(
        "snap-1",
        "/data/clio.db",
        &skipped(SKIP_NO_PRE_BACKUP),
        false,
    );

    assert!(
        prompt.contains("no pre-restore snapshot will be taken"),
        "{prompt}"
    );
    assert!(prompt.contains("no way back"), "{prompt}");
    assert!(
        !prompt.contains("is the way back"),
        "the unconditional promise is the defect: {prompt}"
    );
}

/// The other two reasons a snapshot is skipped: a target that does not exist
/// yet, and one measured empty. `Plan` already knows both.
#[test]
fn a_prompt_for_a_target_with_nothing_to_preserve_does_not_promise_a_way_back() {
    for reason in [SKIP_TARGET_MISSING, SKIP_TARGET_EMPTY] {
        let prompt = prompt_text("snap-1", "/data/clio.db", &skipped(reason), false);

        assert!(
            prompt.contains("no way back"),
            "{reason} must not leave a promise of a way back: {prompt}"
        );
    }
}

/// When a snapshot will be taken the prompt still says so: the conditional
/// clause must not have been dropped along with the unconditional one.
#[test]
fn a_prompt_with_a_pre_restore_snapshot_still_names_the_way_back() {
    let prompt = prompt_text("snap-1", "/data/clio.db", &taking(), false);

    assert!(prompt.contains("is the way back"), "{prompt}");
    assert!(!prompt.contains("no way back"), "{prompt}");
}

/// An archive that carries a key member replaces the live key store, and the
/// prompt says so before the operator confirms rather than only in the report
/// afterwards.
#[test]
fn a_prompt_names_the_key_store_replacement_when_the_archive_carries_one() {
    let with_key = prompt_text("snap-1", "/data/clio.db", &taking(), true);
    let without_key = prompt_text("snap-1", "/data/clio.db", &taking(), false);

    assert!(
        with_key.contains("key store"),
        "the archive carries key material and the prompt must say it: {with_key}"
    );
    assert!(
        !without_key.contains("key store"),
        "an archive with no key member must not claim one: {without_key}"
    );
}

/// A pre-restore decision that will be taken.
fn taking() -> PreRestore {
    PreRestore {
        reason: None,
        id: None,
        path: None,
    }
}

/// A pre-restore decision that will be skipped, for the given reason.
fn skipped(reason: &'static str) -> PreRestore {
    PreRestore {
        reason: Some(reason),
        id: None,
        path: None,
    }
}

/// One report shape, shared by the four durability assertions above.
fn not_durable_report() -> Report {
    Report {
        snapshot_id: "snap-1".to_owned(),
        target: crate::backup_cli::ResolvedTarget {
            url: "/data/clio.db".to_owned(),
            backend: "sqlite".to_owned(),
            database: "clio.db".to_owned(),
            display: "/data/clio.db".to_owned(),
            path: PathBuf::from("/data/clio.db"),
            kms_path: None,
        },
        pre_restore: taking(),
        counts_before: Counts {
            items: 1,
            triples: 0,
            tables: 1,
            banks: Vec::new(),
        },
        counts_after: None,
        counts_incoming: None,
        schema: Schema {
            archive: Some("1".to_owned()),
            action: ACTION_NONE,
        },
        store_schema: None,
        result: RESULT_RESTORED,
        failed_step: None,
        failure: None,
        derived_difference: None,
        derived_index_emptied: false,
        dry_run: false,
        key_restored: false,
        key_replaced: None,
        directory_synced: Some(false),
    }
}

/// The archived counts and the observed ones differ only in a derived table:
/// the run exits 75, `ok` is false because the code is not 0, no step failed,
/// and the line says every user-data row loaded and names the table.
#[test]
fn a_derived_table_difference_exits_75_without_a_failed_step() {
    let mut report = not_durable_report();
    report.directory_synced = Some(true);
    report.result = RESULT_DERIVED_DIFFERENT;
    report.derived_difference = Some(
        "every user-data row was loaded; a table the store rebuilds holds a surplus that neither \
         the reindex a restore prints nor a retry of the restore clears, and the index pipeline \
         reconciles it on its next pass: table \"index_pending\": archive 2 rows, restored 0"
            .to_owned(),
    );

    assert_eq!(exit_code(&report), EXIT_TEMPORARY);
    assert_eq!(exit_code(&report), 75, "the temporary-failure code");
    let json = payload(&report);
    assert_eq!(json["ok"], json!(false), "{json}");
    assert_eq!(json["failed_step"], json!(null), "{json}");
    assert_eq!(
        json["failure"],
        json!(null),
        "no failure on this run: {json}"
    );
    assert_eq!(
        json["derived_difference"],
        json!(
            "every user-data row was loaded; a table the store rebuilds holds a \
               surplus that neither the reindex a restore prints nor a retry of the \
               restore clears, and the index pipeline reconciles it on its next pass: \
               table \"index_pending\": archive 2 rows, restored 0"
        ),
        "{json}"
    );
    assert_eq!(json["result"], json!(RESULT_DERIVED_DIFFERENT), "{json}");
    let text = render(&report);
    assert!(text.contains("result: derived-difference"), "{text}");
    assert!(!text.contains("failed step"), "{text}");
    assert!(text.contains("every user-data row was loaded"), "{text}");
    assert!(text.contains("index_pending"), "{text}");
}

/// The user-data mismatch is unchanged: the count-mismatch result, the failed
/// step, exit 3, and `ok` false.
#[test]
fn a_user_data_count_mismatch_keeps_its_result_and_exit_three() {
    let mut report = not_durable_report();
    report.directory_synced = Some(true);
    report.result = RESULT_COUNT_MISMATCH;
    report.failed_step = Some(FAILED_STEP_COUNTS.to_owned());
    report.failure = Some(
        "the restored table counts do not match the archive's: \
         table \"items\": archive 3 rows, restored 2"
            .to_owned(),
    );

    assert_eq!(exit_code(&report), EXIT_PARTIAL);
    assert_eq!(exit_code(&report), 3);
    let json = payload(&report);
    assert_eq!(json["ok"], json!(false), "{json}");
    assert_eq!(json["failed_step"], json!(FAILED_STEP_COUNTS), "{json}");
    let text = render(&report);
    assert!(
        text.contains("result: count-mismatch (failed step: post-restore count check)"),
        "{text}"
    );
}
