// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving the vector-table DDL fails closed
//! when the sqlite-vec extension is absent from the process.
//!
//! ## Owns
//! - The missing-extension forcing test: the seam's DDL executed on a
//!   connection of a process that never installed the auto-extension, and
//!   the loud `ConfigCorrupt` mapping of the driver refusal
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

//! Missing-extension forcing test.
//!
//! What this test proves: this binary is a fresh process that never calls
//! the seam's registration, so the sqlite-vec module is genuinely absent
//! from every connection created here. The seam's width-parameterized
//! vector-table DDL therefore fails with `no such module`, and the fail-closed
//! error constructor turns that refusal into the loud `ConfigCorrupt` error
//! instead of a silent float-blob downgrade. If the fail-closed contract
//! broke, the mapped code or message assertions here would fail.
//!
//! What this test does not prove: it cannot show a connection failing after
//! the extension was registered, because the auto-extension registration is
//! process-wide and irreversible; and it does not drive the seam's apply
//! wrappers, which are crate-private, so it exercises the public error
//! constructor those wrappers call.

use clio_store::{EmbeddingSpace, sqlite_vec_missing, sqlite_vector_ddl};

#[test]
fn vector_ddl_without_extension_fails_loudly() {
    let space = EmbeddingSpace::new("m", 4).expect("a 4-dim space is valid");
    let ddl = sqlite_vector_ddl(&space).expect("the DDL must generate");
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory sqlite always opens");
    let err = conn
        .execute_batch(&ddl)
        .expect_err("the vector-table DDL must require the sqlite-vec extension");
    assert!(
        err.to_string().contains("no such module"),
        "the refusal must be the absent extension module, not an unrelated error: {err}"
    );
    let mapped = sqlite_vec_missing(err);
    assert_eq!(
        mapped.code,
        clio_types::ErrorCode::ConfigCorrupt,
        "{mapped:?}"
    );
    assert!(
        mapped
            .message
            .contains("silent float-blob mode is not allowed"),
        "{mapped:?}"
    );
}
