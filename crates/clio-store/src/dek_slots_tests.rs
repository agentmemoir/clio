// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the DEK slot operations: ensure,
//! lookup, destroy, conflict-safe creation, and crypto-shred outcomes.
//!
//! ## Owns
//! - In-memory and file-backed ensure/lookup/destroy behavior
//! - Version preservation and the erased-subject outcome
//!
//! ## Does not own
//! - The key-store file format and concurrency (see `local_kms_db_tests`)
//! - The provider crypto round-trips (see `dek_tests`)
//!
//! ## Boundary
//!
//! Tests here break when slot operations change; they never change the
//! implementation.
//!
//! Keep this module focused on the slot operations.

use super::*;
use crate::kms_migrate::install_test_key;
use std::fs;
use std::path::Path;

/// Fresh temp directory for one test, cleaned up by the caller.
///
/// A file-backed slot store seals what it writes, so the shared process key is
/// installed here; without it a test would reach the machine's real key store.
fn temp_dir(tag: &str) -> PathBuf {
    install_test_key();
    let dir = std::env::temp_dir().join(format!("clio-dek-slots-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ensure_creates_and_persists() {
    let dir = temp_dir("ensure");
    let path = dir.join("clio.kms.json");
    let mut store = SlotStore::open(path.clone()).unwrap();
    assert!(store.lookup("s").unwrap().is_none());
    assert_eq!(store.ensure("s").unwrap().0, 1);
    let mut reopened = SlotStore::open(path).unwrap();
    assert!(matches!(
        reopened.lookup("s").unwrap(),
        Some(KeySlot::Active { version: 1, .. })
    ));
    cleanup(&dir);
}

#[test]
fn ensure_refuses_a_destroyed_subject() {
    let mut store = SlotStore::in_memory().unwrap();
    store.ensure("s").unwrap();
    store.destroy("s").unwrap();
    store.destroy("s").unwrap();
    assert_eq!(
        store.ensure("s").unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}

#[test]
fn destroy_keeps_the_existing_version() {
    let dir = temp_dir("version");
    let path = dir.join("clio.kms.json");
    let mut db = KmsDb::open(path.clone()).unwrap();
    db.insert_if_absent("s", 4, &encode_key(&[1u8; KEY_LEN]))
        .unwrap();
    drop(db);
    let mut store = SlotStore::open(path.clone()).unwrap();
    store.destroy("s").unwrap();
    let mut reopened = SlotStore::open(path).unwrap();
    assert!(matches!(
        reopened.lookup("s").unwrap(),
        Some(KeySlot::Destroyed { version: 4 })
    ));
    cleanup(&dir);
}

#[test]
fn in_memory_store_never_touches_a_file() {
    let mut store = SlotStore::in_memory().unwrap();
    store.ensure("s").unwrap();
    store.destroy("s").unwrap();
    assert!(matches!(
        store.lookup("s").unwrap(),
        Some(KeySlot::Destroyed { .. })
    ));
}

#[test]
fn another_instance_shred_is_visible_and_terminal() {
    let dir = temp_dir("shred");
    let path = dir.join("clio.kms.json");
    let mut first = SlotStore::open(path.clone()).unwrap();
    first.ensure("s").unwrap();
    let mut second = SlotStore::open(path).unwrap();
    second.destroy("s").unwrap();
    assert_eq!(
        first.ensure("s").unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    assert!(matches!(
        first.lookup("s").unwrap(),
        Some(KeySlot::Destroyed { .. })
    ));
    cleanup(&dir);
}
