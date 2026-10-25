// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of `restore_authority` that prove a
//! refusal: an archive recording no authority, a foreign or absent salt
//! fingerprint, a key member that changed after classification, and an
//! unreadable key store.
//!
//! ## Owns
//! - The refusal assertions about `classify_archive`
//!
//! ## Does not own
//! - The archive fixtures (`restore_authority_test_support`)
//! - The member-fact cases (`restore_authority_payload_tests`)
//! - The authority check those facts feed (`clio-compliance` owns it)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the refusals of `restore_authority`.

use super::restore_authority_test_support::{
    Scratch, authority, build_sqlite_archive, clean_payload, key_store_with_slot, sqlite_file,
};
use super::{ClassifyRefusal, classify_archive};
use clio_types::ErrorCode;

#[test]
fn an_archive_without_authority_is_refused() {
    let scratch = Scratch::new("no-authority");
    let payload = clean_payload(&scratch);
    let built = build_sqlite_archive(&scratch, &payload, None, None, None);

    let refusal = classify_archive(&built.path, &built.manifest).expect_err("no authority refuses");

    assert!(matches!(refusal, ClassifyRefusal::ArchiveMissingAuthority));
    assert_eq!(refusal.code(), ErrorCode::NotFound);
}

/// An archive whose recorded salt fingerprint differs from the current
/// deployment salt is refused before any member is read.
#[test]
fn an_archive_with_a_foreign_salt_fingerprint_is_refused() {
    let scratch = Scratch::new("salt-mismatch");
    let payload = clean_payload(&scratch);
    let mut authority = authority();
    authority.salt_fingerprint = Some("a-different-salt".to_owned());
    let built = build_sqlite_archive(&scratch, &payload, None, Some(authority), None);

    let refusal =
        classify_archive(&built.path, &built.manifest).expect_err("a salt mismatch refuses");

    assert!(matches!(refusal, ClassifyRefusal::SaltFingerprintMismatch));
    assert_eq!(refusal.code(), ErrorCode::Forbidden);
}

/// An archive whose authority records no salt fingerprint fails closed.
#[test]
fn an_archive_with_an_absent_salt_fingerprint_is_refused() {
    let scratch = Scratch::new("salt-absent");
    let payload = clean_payload(&scratch);
    let mut authority = authority();
    authority.salt_fingerprint = None;
    let built = build_sqlite_archive(&scratch, &payload, None, Some(authority), None);

    let refusal =
        classify_archive(&built.path, &built.manifest).expect_err("an absent fingerprint refuses");

    assert!(matches!(refusal, ClassifyRefusal::SaltFingerprintMismatch));
}

#[test]
fn a_key_member_that_changed_after_classification_is_refused() {
    let scratch = Scratch::new("key-member-changed");
    let payload = clean_payload(&scratch);
    let key = key_store_with_slot(&scratch, "keys.kms.db", "subject-1", "'AAAA'");
    let built = build_sqlite_archive(&scratch, &payload, Some(&key), Some(authority()), None);
    let classified = classify_archive(&built.path, &built.manifest).expect("classify");
    assert!(
        classified.key_member_sha256.is_some(),
        "the classified key member carries a digest"
    );

    // The same extraction passes the proof.
    let same = scratch.join("same");
    std::fs::create_dir_all(&same).expect("create the extraction directory");
    let extracted =
        clio_backup::extract_members(&built.path, &built.manifest, &same).expect("extract");
    assert!(
        classified.verify_key_member(extracted.key.as_ref()).is_ok(),
        "the same key member is accepted"
    );

    // A different key member of the same shape is refused, so a swapped archive
    // cannot be admitted on the strength of the first classification.
    let other_scratch = Scratch::new("key-member-other");
    let other_payload = clean_payload(&other_scratch);
    let other_key = key_store_with_slot(&other_scratch, "keys.kms.db", "subject-1", "'BBBB'");
    let other = build_sqlite_archive(
        &other_scratch,
        &other_payload,
        Some(&other_key),
        Some(authority()),
        None,
    );
    let other_dir = other_scratch.join("other");
    std::fs::create_dir_all(&other_dir).expect("create the other extraction directory");
    let other_extracted =
        clio_backup::extract_members(&other.path, &other.manifest, &other_dir).expect("extract");
    assert!(
        matches!(
            classified.verify_key_member(other_extracted.key.as_ref()),
            Err(ClassifyRefusal::KeyMemberChanged)
        ),
        "a changed key member is refused"
    );
}

#[test]
fn an_unreadable_key_member_is_refused() {
    let scratch = Scratch::new("key-store-unreadable");
    let payload = clean_payload(&scratch);
    let key = scratch.join("keys.kms.db");
    // A SQLite file without the slot table cannot be shown to carry no usable
    // key, so it fails closed instead of yielding an empty live set.
    sqlite_file(&key, &["CREATE TABLE meta (key TEXT, value TEXT)"]);
    let built = build_sqlite_archive(&scratch, &payload, Some(&key), Some(authority()), None);

    let refusal = classify_archive(&built.path, &built.manifest)
        .expect_err("an unreadable key store refuses");

    assert!(matches!(refusal, ClassifyRefusal::KeyStoreUnreadable));
    assert_eq!(refusal.code(), ErrorCode::ConfigCorrupt);
}
