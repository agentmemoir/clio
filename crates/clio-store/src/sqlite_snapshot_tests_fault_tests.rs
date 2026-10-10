// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the snapshot's failure paths: that a
//! copy which cannot be produced is refused and leaves nothing behind, and that
//! a failure after `VACUUM INTO` has already written the staged file removes
//! it instead of leaving a partial payload on disk.
//!
//! ## Owns
//! - The broken-store fixtures and the staged-file removal assertions
//!
//! ## Does not own
//! - The copy, the digest, and the table counts (`sqlite_snapshot`)
//! - The snapshot assertions that no failure path reaches
//!   (see `sqlite_snapshot_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on what a failed snapshot leaves behind.

use super::*;

use rusqlite::Connection;

/// A store whose settings table cannot answer the schema-version read.
///
/// The version is read after the copy, so this store stands for a failure that
/// happens once the staged file already exists.
fn store_with_an_unreadable_version(path: &Path) {
    populated_store(path, 2);
    Connection::open(path)
        .expect("open the source")
        .execute_batch(
            "DROP TABLE schema_settings;
             CREATE TABLE schema_settings (key TEXT PRIMARY KEY);",
        )
        .expect("replace the settings table with one that has no value column");
}

/// A store whose rows cannot be read, which is what stops the copy itself.
///
/// The page number of the `items` table is pointed past the end of the file, so
/// SQLite fails while it is rebuilding the database rather than after it.
fn store_with_an_unreadable_table(path: &Path) {
    populated_store(path, 2);
    let conn = Connection::open(path).expect("open the source");
    conn.execute_batch("PRAGMA writable_schema=ON;")
        .expect("make the schema writable");
    conn.execute_batch(
        "UPDATE sqlite_master SET rootpage = 999999 WHERE type = 'table' AND name = 'items';",
    )
    .expect("point the table past the end of the file");
    conn.execute_batch("PRAGMA writable_schema=OFF;")
        .expect("stop making the schema writable");
}

#[test]
fn a_failure_after_the_copy_removes_the_staged_copy() {
    let scratch = Scratch::new("post-copy-fail");
    let source = scratch.join("clio.db");
    store_with_an_unreadable_version(&source);
    let staged = scratch.join("staged.db");

    let error = snapshot_sqlite_into(&source, &staged)
        .expect_err("a version read that cannot succeed ends the snapshot");

    assert_eq!(error.code, ErrorCode::Internal);
    // SQLite's own warning is that an interrupted `VACUUM INTO` can leave an
    // incomplete file behind, so a run that fails this late must not leave the
    // copy it already wrote for a later run to mistake for a member.
    assert!(
        !staged.exists(),
        "the staged copy is removed when a later step fails"
    );
}

#[test]
fn a_failure_while_copying_leaves_nothing_behind() {
    let scratch = Scratch::new("copy-fail");
    let source = scratch.join("clio.db");
    store_with_an_unreadable_table(&source);
    let staged = scratch.join("staged.db");

    let error =
        snapshot_sqlite_into(&source, &staged).expect_err("a copy that cannot be read fails");

    assert_eq!(error.code, ErrorCode::Internal);
    // SQLite does remove the output of a copy that fails on its own, but that is
    // an implementation detail rather than a promise in its documentation, so
    // the guarantee this asserts has to come from `StagedCopy` either way.
    assert!(
        !staged.exists(),
        "a failed copy leaves no file behind for a later run to mistake for a member"
    );
}
