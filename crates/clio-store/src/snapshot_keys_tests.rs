// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the snapshot key-store member: the
//! subject count for a store with keys, the honest `None` for a store without
//! one, the refusal for a key store that is not a SQLite database, and the
//! guarantee that no key byte or subject identifier escapes.
//!
//! ## Owns
//! - The key-store fixtures and the member-resolution assertions
//!
//! ## Does not own
//! - The SQLite snapshot (see `sqlite_snapshot_tests`)
//! - Archive assembly (see `clio-backup`)
//!
//! ## Boundary
//!
//! Keep this file focused on `snapshot_keys` behavior.

use super::*;
use crate::kms_migrate::install_test_key;
use crate::local_kms_db::KmsDb;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

/// A scratch directory removed when the guard drops.
///
/// Installing the shared process key here is what keeps these tests off the
/// machine's real key store: a file-backed key store seals what it writes.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        install_test_key();
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-store-keys-{tag}"
        )))
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A key store holding `subjects` slots, created through the store's own
/// writer so the file is the real format.
fn key_store_with_subjects(path: &Path, subjects: &[&str]) {
    let mut store = KmsDb::open(path.to_path_buf()).expect("open a key store");
    for (index, subject) in subjects.iter().enumerate() {
        store
            .insert_if_absent(subject, 1, &format!("key-material-{index}"))
            .expect("insert a DEK slot");
    }
}

#[test]
fn a_store_with_two_subjects_reports_both() {
    let scratch = Scratch::new("two");
    let path = scratch.join("clio.db.kms.json");
    key_store_with_subjects(&path, &["subject-a", "subject-b"]);

    let member = key_member_source(Some(&path))
        .expect("resolve the key member")
        .expect("a key store exists");

    assert_eq!(member.path, path);
    assert_eq!(member.subjects, 2);
}

#[test]
fn a_key_store_with_no_file_reports_zero_subjects_through_its_own_accessor() {
    let scratch = Scratch::new("no-file");
    let path = scratch.join("clio.db.kms.json");
    // A store nothing has written to has no key-store file yet, so the
    // accessor reports zero rather than failing.
    let mut store = KmsDb::open(path).expect("open an absent key store");

    assert_eq!(store.subject_count().expect("count an absent key store"), 0);
}

#[test]
fn no_key_store_reports_none_and_never_true() {
    let scratch = Scratch::new("absent");

    // The target keeps its keys in process memory.
    assert_eq!(key_member_source(None).expect("in-memory target"), None);
    // A store nothing has been written to has no key store file yet.
    let missing = scratch.join("clio.db.kms.json");
    assert_eq!(
        key_member_source(Some(&missing)).expect("absent key store"),
        None
    );
}

#[test]
fn a_key_store_path_that_is_not_a_regular_file_is_refused_not_called_absent() {
    let scratch = Scratch::new("path-not-a-file");
    let path = scratch.join("clio.db.kms.json");
    // Something is sitting on the key-store path, so the operator has a key
    // store they expect to travel with the snapshot. Reporting "no key store"
    // here would write an archive that says its keys are in process memory
    // while the key store sits on disk, unread.
    std::fs::create_dir(&path).expect("a directory where the key store should be");

    let error = key_member_source(Some(&path))
        .expect_err("a path that exists but is not a key-store file is refused");

    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("clio.db.kms.json"),
        "the message names the path: {}",
        error.message
    );
}

#[test]
fn a_shredded_subject_still_counts_as_a_slot() {
    let scratch = Scratch::new("shredded");
    let path = scratch.join("clio.db.kms.json");
    key_store_with_subjects(&path, &["subject-a", "subject-b"]);
    let mut store = KmsDb::open(path.clone()).expect("reopen");
    store.shred("subject-b").expect("shred one subject");

    let member = key_member_source(Some(&path))
        .expect("resolve")
        .expect("present");

    assert_eq!(member.subjects, 2, "the slot row survives the shred");
}

#[test]
fn a_key_store_file_that_is_not_a_sqlite_database_is_refused_rather_than_copied() {
    let scratch = Scratch::new("not-sqlite");
    let path = scratch.join("clio.db.kms.json");
    std::fs::write(&path, br#"{"version":1,"slots":{}}"#)
        .expect("write a file that is not a SQLite key store");

    let error = key_member_source(Some(&path))
        .expect_err("a key store that is not a SQLite database is refused");

    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("clio.db.kms.json"),
        "the message names the path: {}",
        error.message
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        br#"{"version":1,"slots":{}}"#.to_vec(),
        "the refusal does not rewrite the key store"
    );
}

#[test]
fn a_truncated_key_store_is_refused() {
    let scratch = Scratch::new("short");
    let path = scratch.join("clio.db.kms.json");
    std::fs::write(&path, b"SQLite").expect("write a short header");

    let error = key_member_source(Some(&path)).expect_err("a short header is refused");

    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
}

#[test]
fn a_damaged_key_store_reports_a_read_failure() {
    let scratch = Scratch::new("damaged");
    let path = scratch.join("clio.db.kms.json");
    key_store_with_subjects(&path, &["subject-a"]);
    // A valid header over bytes that are not a database: the header check
    // passes, the store's own format gate does not.
    let mut bytes = std::fs::read(&path).expect("read the key store");
    bytes.truncate(bytes.len() / 2);
    let truncated = scratch.join("truncated.kms.json");
    let mut header = SQLITE_MAGIC.to_vec();
    header.extend(bytes);
    std::fs::write(&truncated, header).expect("write a damaged key store");

    let error = key_member_source(Some(&truncated)).expect_err("a damaged key store is refused");
    assert!(matches!(
        error.code,
        ErrorCode::Internal | ErrorCode::ConfigCorrupt
    ));
}

#[cfg(unix)]
#[test]
fn an_unreadable_key_store_is_refused_rather_than_counted() {
    let scratch = Scratch::new("unreadable");
    let path = scratch.join("clio.db.kms.json");
    key_store_with_subjects(&path, &["subject-a"]);
    // Root ignores mode bits, so the assertion only runs for a normal user.
    if std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .map(|uids| uids.split_whitespace().next() == Some("0"))
        })
        .unwrap_or(false)
    {
        return;
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod 000");

    let error = key_member_source(Some(&path)).expect_err("an unreadable key store is refused");

    assert_eq!(error.code, ErrorCode::Internal);
    assert!(
        error.message.contains("key store header"),
        "the message names the step that failed: {}",
        error.message
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("restore");
}

#[test]
fn no_key_byte_or_subject_identifier_escapes_the_reported_facts() {
    let scratch = Scratch::new("no-leak");
    let path = scratch.join("clio.db.kms.json");
    key_store_with_subjects(&path, &["subject-a", "subject-b"]);

    let member = key_member_source(Some(&path))
        .expect("resolve")
        .expect("present");
    let rendered = format!("{member:?}");

    for secret in ["subject-a", "subject-b", "key-material-0", "key-material-1"] {
        assert!(!rendered.contains(secret), "the reported facts leaked");
    }
}
