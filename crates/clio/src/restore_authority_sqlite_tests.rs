// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of `restore_authority_sqlite` that
//! prove which SQLite membership fact a payload or key-store row becomes.
//!
//! ## Owns
//! - The SQLite member-fact assertions about `classify_archive`
//!
//! ## Does not own
//! - The archive fixtures (`restore_authority_test_support`)
//! - The Postgres dump-table case (`restore_authority_dump_tests`)
//! - The authority check those facts feed (`clio-compliance` owns it)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on member facts in `restore_authority`.

use super::super::classify_archive;
use super::super::restore_authority_test_support::{
    Scratch, authority, build_sqlite_archive, clean_payload, empty_key_store, key_store_with_slot,
    sqlite_file,
};
use clio_compliance::MemberFacts;

#[test]
fn a_tombstone_hash_becomes_a_reference() {
    let scratch = Scratch::new("tombstone");
    let hash = "a".repeat(64);
    let payload = scratch.join("store.db");
    sqlite_file(
        &payload,
        &[
            "CREATE TABLE compliance_tombstones (subject_hash TEXT NOT NULL)",
            &format!("INSERT INTO compliance_tombstones (subject_hash) VALUES ('{hash}')"),
        ],
    );
    let key = empty_key_store(&scratch);
    let built = build_sqlite_archive(&scratch, &payload, Some(&key), Some(authority()), None);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified
            .members
            .contains(&MemberFacts::Reference { subject_hash: hash }),
        "{:?}",
        classified.members
    );
    assert_eq!(classified.claim.deployment_id, "deploy-1");
    assert_eq!(classified.claim.generation, 3);
    assert_eq!(
        classified.claim.salt_fingerprint,
        Some(clio_compliance::tombstone_salt_fingerprint()),
        "the manifest authority's salt fingerprint reaches the claim"
    );
}

#[test]
fn an_archive_without_a_key_member_fails_closed() {
    let scratch = Scratch::new("no-key-member");
    let hash = "c".repeat(64);
    let payload = scratch.join("store.db");
    sqlite_file(
        &payload,
        &[
            "CREATE TABLE compliance_tombstones (subject_hash TEXT NOT NULL)",
            &format!("INSERT INTO compliance_tombstones (subject_hash) VALUES ('{hash}')"),
        ],
    );
    // Without a key member the restore leaves the target's key store in place,
    // so the tombstone cannot be proven unreadable and is an unproven key.
    let built = build_sqlite_archive(&scratch, &payload, None, Some(authority()), None);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified
            .members
            .contains(&MemberFacts::UnprovenKey { subject_hash: hash }),
        "{:?}",
        classified.members
    );
}

#[test]
fn a_payload_subject_key_id_is_hashed_to_a_reference() {
    let scratch = Scratch::new("subject-key");
    let id = "subject-0007";
    let payload = scratch.join("store.db");
    sqlite_file(
        &payload,
        &[
            "CREATE TABLE subject_keys (subject_id TEXT PRIMARY KEY, dek_ref TEXT)",
            &format!("INSERT INTO subject_keys (subject_id, dek_ref) VALUES ('{id}', 'dek')"),
        ],
    );
    let key = empty_key_store(&scratch);
    let built = build_sqlite_archive(&scratch, &payload, Some(&key), Some(authority()), None);
    let expected = clio_store::sha256_hex(&clio_compliance::tombstone_salt(), id);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified.members.contains(&MemberFacts::Reference {
            subject_hash: expected,
        }),
        "{:?}",
        classified.members
    );
}

#[test]
fn a_key_store_slot_with_a_usable_key_becomes_a_live_key() {
    let scratch = Scratch::new("key-slot");
    let id = "subject-0042";
    let key = key_store_with_slot(&scratch, "keys.kms.db", id, "'AAAA'");
    let payload = clean_payload(&scratch);
    let built = build_sqlite_archive(&scratch, &payload, Some(&key), Some(authority()), None);
    let expected = clio_store::sha256_hex(&clio_compliance::tombstone_salt(), id);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified.members.contains(&MemberFacts::LiveKey {
            subject_hash: expected,
        }),
        "{:?}",
        classified.members
    );
}

#[test]
fn a_destroyed_key_slot_is_a_content_free_reference() {
    let scratch = Scratch::new("destroyed-key-slot");
    let id = "subject-0043";
    let key = key_store_with_slot(&scratch, "keys.kms.db", id, "NULL");
    let payload = clean_payload(&scratch);
    let built = build_sqlite_archive(&scratch, &payload, Some(&key), Some(authority()), None);
    let expected = clio_store::sha256_hex(&clio_compliance::tombstone_salt(), id);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified.members.contains(&MemberFacts::Reference {
            subject_hash: expected,
        }),
        "{:?}",
        classified.members
    );
    assert!(
        classified
            .members
            .iter()
            .all(|member| !matches!(member, MemberFacts::LiveKey { .. })),
        "a destroyed slot carries no usable key: {:?}",
        classified.members
    );
}

/// A payload subject whose archive key slot is still usable is a live key, so a
/// pre-erasure archive that carries the key refuses even from the payload.
#[test]
fn a_payload_subject_key_id_with_a_usable_slot_becomes_a_live_key() {
    let scratch = Scratch::new("payload-live-key");
    let id = "subject-0044";
    let payload = scratch.join("store.db");
    sqlite_file(
        &payload,
        &[
            "CREATE TABLE subject_keys (subject_id TEXT PRIMARY KEY, dek_ref TEXT)",
            &format!("INSERT INTO subject_keys (subject_id, dek_ref) VALUES ('{id}', 'dek')"),
        ],
    );
    let key = key_store_with_slot(&scratch, "keys.kms.db", id, "'AAAA'");
    let built = build_sqlite_archive(&scratch, &payload, Some(&key), Some(authority()), None);
    let expected = clio_store::sha256_hex(&clio_compliance::tombstone_salt(), id);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified.members.contains(&MemberFacts::LiveKey {
            subject_hash: expected,
        }),
        "{:?}",
        classified.members
    );
}
