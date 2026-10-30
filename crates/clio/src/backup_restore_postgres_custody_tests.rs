// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the restore route's copy-custody refusals against a
//! Postgres target: a restore with no custody row behind the copy, a copy
//! whose recomputed digest differs from the row's, and a restorer that is not
//! the row's receiver all refuse before anything is placed.
//!
//! ## Owns
//! - The three refusal assertions at the route boundary over a real scoped
//!   schema pair
//!
//! ## Does not own
//! - The gate's rules and refusal set (`clio-compliance` owns them)
//! - The shared fixtures (see `backup_restore_postgres_tests` and
//!   `backup_restore_custody`'s route tests)
//!
//! ## Boundary
//!
//! Tests here break when a refusal stops running before placement or its
//! wording loses the operator's next step.
//!
//! Test-only, and every test serializes on the shared Postgres test lock.
//! Never compiled into production builds.

use super::tests::{base_url, seed, snapshot};
use crate::backup_cli_fixtures::{Scratch, run_json_in_with};
use crate::backup_restore_custody::route_tests::{
    archive_path, clear_copy_custody, seed_custody_row,
};
use clio_store::{ScopedSchema, lock_pg_test, postgres_target_state};
use std::path::Path;
use std::process::ExitCode;

/// A digest no archive recomputes to, in the delivery expression's shape.
const OTHER_DIGEST: &str =
    "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// An actor the restore route never acts as.
const OTHER_RECEIVER: &str = "agent";

/// One source schema seeded with items, one empty target schema, and the
/// snapshot id of the source's archive, whose custody row the fixture seeds.
fn fixture(label: &str) -> (Scratch, ScopedSchema, ScopedSchema, String, String) {
    let base = base_url();
    let a = ScopedSchema::new(&base);
    let b = ScopedSchema::new(&base);
    let scratch = Scratch::new(label);
    let kms_a = scratch.join("kms-a.json").display().to_string();
    let kms_b = scratch.join("kms-b.json").display().to_string();
    seed(&a, Path::new(&kms_a), 2);
    seed(&b, Path::new(&kms_b), 0);
    let id = snapshot(&scratch, &a, &kms_a);
    (scratch, a, b, id, kms_b)
}

/// Restore `id` into the target schema, reporting the exit code and stderr.
fn restore_into(
    scratch: &Scratch,
    id: &str,
    target: &ScopedSchema,
    kms_b: &str,
) -> (std::process::ExitCode, String) {
    let (code, _, err) = run_json_in_with(
        scratch,
        &["restore", id, "--yes", "--allow-missing-keys"],
        &[
            ("CLIO_DATABASE_URL", target.url()),
            ("CLIO_KMS_FILE", kms_b),
        ],
    );
    (code, err)
}

#[test]
fn a_postgres_restore_without_a_custody_row_refuses_before_placement() {
    let _guard = lock_pg_test();
    let (scratch, _a, b, id, kms_b) = fixture("restore-pg-custody-missing");
    clear_copy_custody(&scratch);

    let (code, err) = restore_into(&scratch, &id, &b, &kms_b);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("no copy-custody record"), "{err}");
    let state = postgres_target_state(b.url()).expect("measure the target");
    assert_eq!(
        state.count_of("items"),
        Some(0),
        "the target store is untouched"
    );
}

#[test]
fn a_postgres_restore_whose_row_records_a_different_digest_refuses_before_placement() {
    let _guard = lock_pg_test();
    let (scratch, _a, b, id, kms_b) = fixture("restore-pg-custody-digest");
    let archive = archive_path(&scratch, &id);
    clear_copy_custody(&scratch);
    seed_custody_row(&scratch, &id, &archive, Some(OTHER_DIGEST), None);

    let (code, err) = restore_into(&scratch, &id, &b, &kms_b);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(
        err.contains("digest does not match the digest its custody record holds"),
        "{err}"
    );
    let state = postgres_target_state(b.url()).expect("measure the target");
    assert_eq!(
        state.count_of("items"),
        Some(0),
        "the target store is untouched"
    );
}

#[test]
fn a_postgres_restore_by_an_actor_other_than_the_row_receiver_refuses_before_placement() {
    let _guard = lock_pg_test();
    let (scratch, _a, b, id, kms_b) = fixture("restore-pg-custody-receiver");
    let archive = archive_path(&scratch, &id);
    clear_copy_custody(&scratch);
    seed_custody_row(&scratch, &id, &archive, None, Some(OTHER_RECEIVER));

    let (code, err) = restore_into(&scratch, &id, &b, &kms_b);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(
        err.contains("not the receiver the copy's custody record names"),
        "{err}"
    );
    let state = postgres_target_state(b.url()).expect("measure the target");
    assert_eq!(
        state.count_of("items"),
        Some(0),
        "the target store is untouched"
    );
}
