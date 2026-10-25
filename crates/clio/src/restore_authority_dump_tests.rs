// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of `restore_authority_dump` that
//! prove which membership fact a Postgres subject table becomes, and that a
//! content table becomes none.
//!
//! ## Owns
//! - The Postgres dump-table assertions about `classify_archive`
//!
//! ## Does not own
//! - The archive fixtures (`restore_authority_test_support`)
//! - The SQLite member-fact cases (`restore_authority_sqlite_tests`)
//! - The authority check those facts feed (`clio-compliance` owns it)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on Postgres dump tables in `restore_authority`.

use super::super::classify_archive;
use super::super::restore_authority_test_support::{
    Scratch, build_postgres_archive, empty_key_store,
};
use clio_compliance::MemberFacts;

#[test]
fn a_postgres_subject_table_becomes_a_reference_and_content_does_not() {
    let scratch = Scratch::new("postgres");
    let hash = "b".repeat(64);
    let tombstone_copy = format!("{hash}\tCOMPLIANCE_CRYPTO_SHRED\n");
    let subject_id = "subject-0009";
    let subject_copy = format!("{subject_id}\tdek\tactive\n");
    let key = empty_key_store(&scratch);
    let built = build_postgres_archive(
        &scratch,
        &[
            (
                "compliance_tombstones",
                &["subject_hash", "reason_code"],
                tombstone_copy.as_bytes(),
            ),
            ("items", &["id", "subject_id"], b"item-1\tsubject-0009\n"),
            (
                "subject_keys",
                &["subject_id", "dek_ref", "status"],
                subject_copy.as_bytes(),
            ),
            ("triples", &["id"], b"triple-1\n"),
        ],
        Some(&key),
    );
    let expected = clio_store::sha256_hex(&clio_compliance::tombstone_salt(), subject_id);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified
            .members
            .contains(&MemberFacts::Reference { subject_hash: hash }),
        "{:?}",
        classified.members
    );
    assert!(
        classified.members.contains(&MemberFacts::Reference {
            subject_hash: expected,
        }),
        "{:?}",
        classified.members
    );
    // The subject key and the item row name the same subject, so one hash
    // covers both, and the content table names no subject.
    assert!(
        classified
            .members
            .iter()
            .filter(|member| matches!(member, MemberFacts::None))
            .count()
            >= 3,
        "{:?}",
        classified.members
    );
}
