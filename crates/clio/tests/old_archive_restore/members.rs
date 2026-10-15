// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for inspecting a fresh same-deployment archive
//! taken after a synthetic erasure, on both backends: what the denied subject's
//! retained references are, what reads back, and what happens to an archive
//! member the manifest does not classify.
//!
//! ## Owns
//! - The post-erasure archive inspection and its retained-reference log
//! - The hand-built archive carrying an unclassifiable member
//!
//! ## Does not own
//! - The archive assembly and read-back probes (see `support`)
//! - The pre-erasure per-route matrices (see `sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to what a post-erasure archive is inspected for should normally
//! require changes here. Changes to a restore primitive should not.
//!
//! Keep this module focused on post-erasure archive inspection.

use crate::support::{
    Archive, CONTROL_ITEM, CONTROL_SUBJECT, DENIED_ITEM, DENIED_SUBJECT, Readback, Scratch,
    archive_postgres, archive_sqlite, erase_subject, observe, open_pg, open_sqlite, pg_url,
    postgres_reachable, readback, subject_key_destroyed, write_item,
};
use crate::support_pg::require_postgres;
use clio::store::{
    ComplianceStore, ScopedSchema, lock_pg_test, postgres_target_state, sqlite_file_state,
};
use clio_backup::container::archive_from_raw_members_for_tests;
use clio_backup::{KEY_MEMBER, MANIFEST_MEMBER, extract_members, read_manifest};
use std::path::{Path, PathBuf};

/// Build an archive from the archive's own members plus one undeclared member.
///
/// The container is hand-built because a production create only writes declared
/// members; the extra member is exactly the unclassifiable case.
pub(crate) fn raw_with_unclassifiable(archive: &Archive, dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).expect("create the raw-build directory");
    let extracted =
        extract_members(&archive.path, &archive.manifest, dir).expect("extract the archive");
    let mut members: Vec<(String, Vec<u8>)> = vec![
        (MANIFEST_MEMBER.to_owned(), archive.manifest.to_json_bytes()),
        (
            extracted.payload.member.clone(),
            std::fs::read(extracted.payload.file.path()).expect("read the payload"),
        ),
    ];
    for table in &extracted.dump_tables {
        members.push((
            table.member.clone(),
            std::fs::read(table.file.path()).expect("read a table member"),
        ));
    }
    if let Some(key) = &extracted.key {
        members.push((
            KEY_MEMBER.to_owned(),
            std::fs::read(key.file.path()).expect("read the key member"),
        ));
    }
    members.push((
        "unclassifiable.member".to_owned(),
        b"no manifest entry describes this member".to_vec(),
    ));
    let refs: Vec<(&str, &[u8])> = members
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    let path = dir.join("unclassifiable.cliobak.tar.gz");
    std::fs::write(&path, archive_from_raw_members_for_tests(&refs))
        .expect("write the raw archive");
    path
}

/// Inspect the post-erasure archive's members and an unclassifiable member.
fn inspect_unclassifiable(archive: &Archive, label: &str) {
    let dir = archive.path.parent().expect("archive parent").join("raw");
    let raw = raw_with_unclassifiable(archive, &dir);
    let manifest = read_manifest(&raw).expect("the hand-built manifest reads back");
    let extracted = extract_members(&raw, &manifest, &dir)
        .expect("an undeclared member is skipped, not refused, before any guard");
    assert!(
        extracted.key.is_some(),
        "the key member is still taken from an archive with an unclassifiable member"
    );
    println!(
        "OBS backend={label} route=unclassifiable-member outcome=skipped-pre-guard archive={}",
        archive.id
    );
}

#[test]
fn sqlite_post_erasure_archive_retains_denied_references() {
    let scratch = Scratch::new("sqlite-t05");
    let db = scratch.join("a/clio.db");
    let kms = scratch.join("a/clio.kms.json");
    std::fs::create_dir_all(db.parent().expect("db parent")).expect("create a");
    let store = open_sqlite(&db, &kms);
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist");
    write_item(
        &store,
        CONTROL_ITEM,
        CONTROL_SUBJECT,
        "control sentinel gist",
    );
    erase_subject(&store, DENIED_SUBJECT, "req-1");
    drop(store);

    // A fresh, same-deployment archive taken after the erasure.
    let archive = archive_sqlite(&scratch.join("a/backups"), &db, &kms, "sqlite-post-erase");

    let target_db = scratch.join("t/clio.db");
    let target_kms = scratch.join("t/clio.kms.json");
    crate::support::restore_sqlite(&archive, &target_db, &target_kms, true);
    let store = open_sqlite(&target_db, &target_kms);
    let denied = readback(&store, DENIED_ITEM);
    let control = readback(&store, CONTROL_ITEM);
    let key_destroyed = subject_key_destroyed(&store, DENIED_SUBJECT);
    let tombstones = store
        .list_tombstones(crate::support::BANK)
        .expect("list tombstones");
    let items_after = sqlite_file_state(&target_db)
        .expect("measure the restored store")
        .count_of("items");
    observe(
        "sqlite",
        "post-erase-archive",
        &archive,
        &denied,
        key_destroyed,
    );

    assert_eq!(
        denied,
        Readback::DestroyedKey,
        "the ciphertext-only row survives but its DEK is destroyed"
    );
    assert_eq!(
        control,
        Readback::Readable,
        "the control subject stays readable"
    );
    assert!(key_destroyed, "the destroyed key slot is retained");
    assert!(
        !tombstones.is_empty(),
        "the archive retains the content-free tombstone rows"
    );
    assert_eq!(
        items_after,
        Some(2),
        "the denied subject's ciphertext-only row is retained alongside the control"
    );
    println!(
        "RETAINED sqlite tombstones={} retained_items={items_after:?} keys_included={}",
        tombstones.len(),
        archive.manifest.keys.included
    );

    inspect_unclassifiable(&archive, "sqlite");
}

#[test]
fn postgres_post_erasure_archive_retains_denied_references() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg_test();
    let scratch = Scratch::new("pg-t05");
    let a = ScopedSchema::new(&base);
    let kms = scratch.join("a/clio.kms.json");
    let store = open_pg(a.url(), &kms);
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist");
    write_item(
        &store,
        CONTROL_ITEM,
        CONTROL_SUBJECT,
        "control sentinel gist",
    );
    erase_subject(&store, DENIED_SUBJECT, "req-1");
    drop(store);

    let archive = archive_postgres(&scratch.join("a/backups"), a.url(), &kms, "pg-post-erase");

    let target = ScopedSchema::new(&base);
    let target_kms = scratch.join("t/clio.kms.json");
    drop(open_pg(target.url(), &target_kms));
    crate::support::restore_pg(
        &archive,
        &target,
        &target_kms,
        &scratch.join("t/extract"),
        true,
    );
    let store = open_pg(target.url(), &target_kms);
    let denied = readback(&store, DENIED_ITEM);
    let control = readback(&store, CONTROL_ITEM);
    let key_destroyed = subject_key_destroyed(&store, DENIED_SUBJECT);
    let tombstones = store
        .list_tombstones(crate::support::BANK)
        .expect("list tombstones");
    drop(store);
    let items_after: Option<u64> = postgres_target_state(target.url())
        .expect("measure the restored schema")
        .count_of("items");
    observe(
        "postgres",
        "post-erase-archive",
        &archive,
        &denied,
        key_destroyed,
    );

    assert_eq!(
        denied,
        Readback::DestroyedKey,
        "the ciphertext-only row survives but its DEK is destroyed"
    );
    assert_eq!(
        control,
        Readback::Readable,
        "the control subject stays readable"
    );
    assert!(key_destroyed, "the destroyed key slot is retained");
    assert!(
        !tombstones.is_empty(),
        "the archive retains the content-free tombstone rows"
    );
    assert_eq!(
        items_after,
        Some(2),
        "the denied subject's ciphertext-only row is retained alongside the control"
    );
    println!(
        "RETAINED postgres tombstones={} retained_items={items_after:?} keys_included={}",
        tombstones.len(),
        archive.manifest.keys.included
    );

    inspect_unclassifiable(&archive, "postgres");
}
