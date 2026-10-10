// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing store installation: temp-file
//! naming, parent-directory creation, and the atomic empty-store install.
//!
//! ## Owns
//! - The installation behavior assertions
//!
//! ## Does not own
//! - The key-store rows and format gate (see `local_kms_db_tests`)
//!
//! ## Boundary
//!
//! Changes to the installation behavior assertions should normally require
//! changes here.
//! Changes to the key-store rows and format gate should not require changes
//! here.
//!
//! Keep this file focused on store installation behavior.

use super::*;
use crate::local_kms_db::{SQLITE_MAGIC, insert_slot_if_absent};
use std::fs;
use std::io::Read as _;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

/// Fresh temp directory for one test, cleaned up by the caller.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("clio-kms-install-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[test]
fn temp_paths_are_unique_and_stay_beside_the_store() {
    let dir = temp_dir("temp-names");
    let path = dir.join("clio.kms.json");

    let first = temp_path(&path, "create");
    let second = temp_path(&path, "wrap");

    assert_ne!(
        first, second,
        "concurrent creators must never share a temp file"
    );
    assert_eq!(first.parent(), Some(dir.as_path()));
    assert!(first.to_string_lossy().contains(".create"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn create_parent_dir_makes_missing_parents_and_is_idempotent() {
    let dir = temp_dir("parents");
    let path = dir.join("deep").join("keys").join("clio.kms.json");

    create_parent_dir(&path).unwrap();
    assert!(path.parent().unwrap().is_dir());
    create_parent_dir(&path).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn install_empty_store_installs_once_and_keeps_the_winner() {
    let dir = temp_dir("install");
    let path = dir.join("clio.kms.json");

    install_empty_store(&path).unwrap();
    let mut magic = [0u8; 16];
    fs::File::open(&path)
        .unwrap()
        .read_exact(&mut magic)
        .unwrap();
    assert_eq!(&magic, SQLITE_MAGIC);
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600,
        "an installed store is owner-only"
    );

    // A second creator keeps the store that is already in place.
    let conn = Connection::open(&path).unwrap();
    insert_slot_if_absent(&conn, "s", 1, "k").unwrap();
    drop(conn);
    install_empty_store(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    let slots: i64 = conn
        .query_row("SELECT COUNT(*) FROM slots", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        slots, 1,
        "a second install must not replace the winner's store"
    );
    fs::remove_dir_all(&dir).unwrap();
}
