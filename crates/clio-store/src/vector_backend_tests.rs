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

#[test]
fn knn_scan_returns_rows_nearest_first_up_to_the_fetch_limit() {
    let conn = wired_connection();
    apply_space_ddl(&conn, &EmbeddingSpace::new("m", 4).unwrap())
        .expect("space DDL must apply once registered");
    let insert = |item: &str, bank: &str, v: [f32; 4]| {
        conn.execute(
            "INSERT INTO item_embeddings
             (item_id, embedding, bank_id, model_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'm', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            rusqlite::params![item, v.as_bytes(), bank],
        )
        .expect("insert a vector row");
    };
    insert("near", "b1", [1.0, 0.0, 0.0, 0.0]);
    insert("mid", "b1", [0.7, 0.7, 0.0, 0.0]);
    insert("far", "b2", [0.0, 1.0, 0.0, 0.0]);
    let query = [1.0_f32, 0.0, 0.0, 0.0];
    let limited = knn_scan(&conn, &query, 2).expect("scan at fetch_k = 2");
    let ids: Vec<&str> = limited.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["near", "mid"], "{limited:?}");
    let widened = knn_scan(&conn, &query, 3).expect("scan at fetch_k = 3");
    let ids: Vec<&str> = widened.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["near", "mid", "far"], "{widened:?}");
    let mut distances = widened.iter().map(|(_, d, _)| *d).collect::<Vec<_>>();
    distances.sort_by(|a, b| a.partial_cmp(b).expect("finite distances"));
    assert!(distances.windows(2).all(|w| w[0] < w[1]), "{distances:?}");
}