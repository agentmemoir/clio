// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for proving the SQLite vector DDL needs the extension.
//!
//! ## Owns
//! - Missing-extension forcing test (fresh process, extension never registered)
//!
//! ## Does not own
//! - Extension registration (see `vector_backend`)
//! - Migration orchestration (see `migrate`)
//!
//! ## Boundary
//!
//! Changes to vector DDL requirements should normally require changes here.
//! Changes to registration mechanics should not require changes here.
//!
//! Keep this module focused on the missing-extension loud-failure contract.

//! Missing-extension forcing test (BUG-007).
//!
//! This binary never registers the sqlite-vec auto-extension, so the
//! checked-in vec0 DDL must fail here — proving the DDL genuinely requires
//! the extension (no pretend float-blob mode) and that the failure maps to
//! the loud `ConfigCorrupt` error instead of a silent downgrade.

const VEC_DDL: &str = include_str!("../../../sql/002_vectors_sqlite.sql");

#[test]
fn vector_ddl_without_extension_fails_loudly() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let err = conn
        .execute_batch(VEC_DDL)
        .expect_err("vec0 DDL must require the sqlite-vec extension");
    let mapped = clio_store::sqlite_vec_missing(err);
    assert_eq!(mapped.code, clio_types::ErrorCode::ConfigCorrupt);
    assert!(
        mapped
            .message
            .contains("silent float-blob mode is not allowed")
    );
}
