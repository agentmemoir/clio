// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite vector backend seam.
//!
//! ## Owns
//! - Registration outcome mapping and auto-extension return-code interpretation
//! - Width-parameterized SQLite vector DDL text and its failure contract
//! - The fail-closed apply/verify wrappers on a live connection
//!
//! ## Does not own
//! - Production behaviour
//! - Postgres DDL tests (`vector_ddl`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::migrate::{SQL_COMPLIANCE_SLOTS, SQL_CORE};
use clio_types::ErrorCode;
use rusqlite::Connection;

/// A connection with the backend registered and the static SQL applied.
fn wired_connection() -> Connection {
    ensure_registered().expect("vector backend registration must succeed");
    let conn = Connection::open_in_memory().expect("in-memory sqlite always opens");
    conn.execute_batch(&format!("{SQL_CORE}{SQL_COMPLIANCE_SLOTS}"))
        .expect("core bootstrap on a bare connection");
    conn
}

#[test]
fn registration_helpers_cover_both_arms() {
    assert!(map_register_outcome(&Ok(())).is_ok());
    assert_eq!(
        map_register_outcome(&Err("boom".into()))
            .unwrap_err()
            .code,
        ErrorCode::ConfigCorrupt
    );
    assert!(interpret_auto_extension_rc(0).is_ok());
    assert!(interpret_auto_extension_rc(1).is_err());
    assert!(ensure_registered().is_ok());
}

#[test]
fn sqlite_ddl_carries_width_and_aux_columns() {
    let ddl = sqlite_vector_ddl(&EmbeddingSpace::new("m", 1536).unwrap()).unwrap();
    assert!(ddl.contains("USING vec0"), "{ddl}");
    assert!(
        ddl.contains("float[1536] distance_metric = cosine"),
        "{ddl}"
    );
    assert!(ddl.contains("+model_id"), "{ddl}");
    assert_eq!(
        sqlite_vector_ddl(&EmbeddingSpace {
            model_id: "m".into(),
            dims: 0
        })
        .unwrap_err()
        .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        sqlite_vector_ddl(&EmbeddingSpace {
            model_id: "m".into(),
            dims: 99_999
        })
        .unwrap_err()
        .code,
        ErrorCode::OutOfRange
    );
}

#[test]
fn space_ddl_applies_on_a_wired_connection() {
    let conn = wired_connection();
    let space = EmbeddingSpace::new("m", 1536).unwrap();
    apply_space_ddl(&conn, &space).expect("space DDL must apply once registered");
    let dims: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM item_embeddings",
            [],
            |row| row.get(0),
        )
        .expect("the vector table must exist after the space DDL");
    assert_eq!(dims, 0);
}

#[test]
fn space_ddl_refuses_an_out_of_range_width() {
    let conn = wired_connection();
    let err = apply_space_ddl(&conn, &EmbeddingSpace {
        model_id: "m".into(),
        dims: 0,
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
}

#[test]
fn static_vectors_sql_applies_after_core() {
    let conn = wired_connection();
    apply_static_vectors(&conn).expect("static vectors SQL must apply after core");
}

#[test]
fn static_vectors_sql_fails_closed_before_core() {
    ensure_registered().expect("vector backend registration must succeed");
    let conn = Connection::open_in_memory().expect("in-memory sqlite always opens");
    let err = apply_static_vectors(&conn).unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}

#[test]
fn verify_vec_version_answers_when_registered() {
    let conn = wired_connection();
    verify_vec_version(&conn).expect("the backend must answer vec_version once registered");
}