// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the one mapping from a placement's observed counts to the
//! answer its report carries: which result the observation produces, which step
//! it names as failed, and whether a failure text or a derived difference
//! travels beside it.
//!
//! ## Owns
//! - A derived-only surplus: the derived result, no failed step, no failure
//! - The user-data differences and a failed key store, which do name a step
//! - Equal counts, which name nothing
//!
//! ## Does not own
//! - The comparison's own arms and wording (see `backup_restore_counts_tests`)
//! - The database an observation comes from (see the Postgres-backed tests in
//!   `backup_restore_postgres_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every test here is pure: it
//! starts from an observation and a hand-built dump meta, so it needs no
//! database and reads nothing from the host it runs on.

use super::{Observation, observation};
use crate::backup_restore_flow::{
    FAILED_STEP_COUNTS, FAILED_STEP_KEY_SWAP, RESULT_COUNT_MISMATCH, RESULT_DERIVED_DIFFERENT,
    RESULT_PARTIAL, RESULT_RESTORED,
};
use clio_backup::dump_meta::{DUMP_FORMAT_VERSION, DUMP_KIND, DumpColumn, DumpMeta, DumpTable};
use clio_store::{PG_CONSISTENCY, PG_METHOD};

/// One archive table with the row count the dump recorded for it.
fn table(name: &str, rows: u64) -> DumpTable {
    DumpTable {
        table: name.to_owned(),
        member: format!("dump/tables/{name}.txt"),
        bytes: 4,
        sha256: "0".repeat(64),
        rows,
        columns: vec![DumpColumn {
            name: "id".to_owned(),
            ordinal: 1,
            type_name: "bigint".to_owned(),
        }],
    }
}

/// The archive an observation is compared against: three user-data rows and two
/// queued rows in the table the store rebuilds.
fn two_table_meta() -> DumpMeta {
    DumpMeta {
        kind: DUMP_KIND.to_owned(),
        format_version: DUMP_FORMAT_VERSION,
        backend: "postgres".to_owned(),
        schema: "public".to_owned(),
        database: "clio".to_owned(),
        extensions: Vec::new(),
        banks: Vec::new(),
        method: PG_METHOD.to_owned(),
        consistency: PG_CONSISTENCY.to_owned(),
        tables: vec![table("items", 3), table("index_pending", 2)],
    }
}

/// The counts a placement observed, in the shape the store half reports them.
fn observed(items: u64, derived: u64) -> Vec<(String, u64)> {
    vec![
        ("items".to_owned(), items),
        ("index_pending".to_owned(), derived),
    ]
}

/// The answer one observation of the archive above produces.
fn answer(items: u64, derived: u64, key_failure: Option<&str>) -> Observation {
    observation(&two_table_meta(), &observed(items, derived), key_failure)
}

/// A table the store rebuilds that came back with more rows than the archive
/// recorded is the one benign difference: the result says so, no step is named
/// as failed, and no failure text is carried, so a consumer keying on `failure`
/// cannot mistake it for a run that lost data.
#[test]
fn a_derived_only_surplus_names_no_failed_step_and_no_failure() {
    let answer = answer(3, 5, None);

    assert_eq!(answer.result, RESULT_DERIVED_DIFFERENT);
    assert_eq!(answer.failed_step, None, "no step failed");
    assert_eq!(answer.failure, None, "nothing failed and nothing was lost");
    let difference = answer
        .derived_difference
        .expect("the difference is carried in its own field");
    assert!(difference.contains("index_pending"), "{difference}");
    assert!(
        difference.contains("archive 2 rows, restored 5"),
        "{difference}"
    );
}

/// A user-data table with a different count is the failed count check, and the
/// mapping names the step and the text: the observation does not become the
/// benign difference.
#[test]
fn a_user_data_difference_names_the_count_step_and_the_failure() {
    let answer = answer(2, 2, None);

    assert_eq!(answer.result, RESULT_COUNT_MISMATCH);
    assert_eq!(answer.failed_step.as_deref(), Some(FAILED_STEP_COUNTS));
    let failure = answer
        .failure
        .as_deref()
        .expect("the lost row is worded as a failure");
    assert!(failure.contains("items"), "{failure}");
    assert_eq!(answer.derived_difference, None);
}

/// A derived surplus beside a key store that was not placed is a partial
/// restore that names only the key step: the derived difference never becomes a
/// failure, and the key failure does.
#[test]
fn a_derived_surplus_beside_a_failed_key_store_is_partial() {
    let answer = answer(3, 5, Some("the staged key store could not be placed"));

    assert_eq!(answer.result, RESULT_PARTIAL);
    assert_eq!(answer.failed_step.as_deref(), Some(FAILED_STEP_KEY_SWAP));
    let failure = answer.failure.expect("the key failure is worded");
    assert!(failure.contains("key store"), "{failure}");
    assert!(
        answer.derived_difference.is_some(),
        "the derived difference is still carried"
    );
}

/// Equal counts on both tables are the clean restore: the restored result and no
/// field behind it.
#[test]
fn equal_counts_name_nothing() {
    let answer = answer(3, 2, None);

    assert_eq!(answer.result, RESULT_RESTORED);
    assert_eq!(answer.failed_step, None);
    assert_eq!(answer.failure, None);
    assert_eq!(answer.derived_difference, None);
}
