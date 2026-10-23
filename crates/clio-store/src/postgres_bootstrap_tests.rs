// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres store's own bootstrap behavior:
//! the timestamp rewrite an open runs on stored epoch-millis item columns, the
//! confirmed width switch that recreates the vector table, and the width gate
//! that refuses a `halfvec` index on a pgvector too old to serve it.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.
//!
//! - An open rewriting epoch-millis item timestamps into canonical seconds
//! - The confirmed space switch recreating the vector table at the new width
//! - The width-appropriate distance expression, and the pre-`halfvec` refusal
//!
//! ## Does not own
//! - The bootstrap advisory lock (`postgres_bootstrap_lock_tests`)
//! - Space identity or width policy (`embedding_space`) and DDL text (`vector_ddl`)
//! - The rewrite rule itself (`migrate_ts_backfill`)
//!
//! ## Boundary
//!
//! Changes to what an open does to an existing database belong here. Changes to
//! the width → index-mode calibration do not.
//!
//! Every case runs in a private schema (`pg_scoped_schema`) so the shared
//! Compose database is never disturbed, and the schema is dropped on exit.

use super::{apply_vector_ddl_with, distance_expr};
use crate::embedding_space::EmbeddingSpace;
use crate::embedding_space::embedding_space_pg_space_tests::{base_url, item};
use crate::pg_scoped_schema::ScopedSchema;
use crate::store::Store;
use clio_types::ErrorCode;

#[test]
fn an_open_rewrites_epoch_millis_item_timestamps() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&base_url());
    let store = schema.open();
    store
        .create_memory_item(&item("b1-i1", "b1"), "actor")
        .unwrap();
    // The shape a previous build wrote: the item's creation time as epoch millis.
    schema
        .execute(
            "UPDATE items SET created_at = '1700000000000', updated_at = '1700000000000'
             WHERE id = 'b1-i1'",
        )
        .unwrap();

    // A later open runs the rewrite, so the stored columns read back canonical.
    // The row is read with raw SQL because the second open brings its own
    // process-local KMS, which cannot unwrap the first store's content.
    let _reopened = schema.open();
    assert_eq!(
        schema
            .query_value("SELECT created_at FROM items WHERE id = 'b1-i1'")
            .unwrap(),
        Some("2023-11-14T22:13:20Z".to_owned())
    );
    assert_eq!(
        schema
            .query_value("SELECT updated_at FROM items WHERE id = 'b1-i1'")
            .unwrap(),
        Some("2023-11-14T22:13:20Z".to_owned())
    );
    // A column the rewrite leaves alone keeps its stored value.
    assert_eq!(
        schema
            .query_value("SELECT tx_from FROM items WHERE id = 'b1-i1'")
            .unwrap(),
        Some("t0".to_owned())
    );
}

#[test]
fn a_confirmed_switch_recreates_the_vector_table_at_the_new_width() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&base_url());
    let mut store = schema.open();
    store
        .create_memory_item(&item("b1-i1", "b1"), "actor")
        .unwrap();
    let target = EmbeddingSpace::new("local/wide", 768).unwrap();

    let report = store.apply_configured_space(&target, true).unwrap();
    assert!(report.switched);
    assert_eq!(report.from, EmbeddingSpace::default_space());
    assert_eq!(report.to, target);
    assert_eq!(store.space().dims, 768);
    // The switch recreates derived vectors only: the plain rows are untouched.
    assert!(store.get_memory_item("b1", "b1-i1").unwrap().is_some());
    // The same space again is a no-op, not a second destructive switch.
    let again = store.apply_configured_space(&target, true).unwrap();
    assert!(!again.switched);
    assert_eq!(again.purged_rows, 0);
}

#[test]
fn the_distance_expression_casts_only_above_the_plain_vector_ceiling() {
    let narrow = EmbeddingSpace::new("local/narrow", 1536).unwrap();
    assert_eq!(distance_expr(&narrow, "e"), "e.embedding <=> $1");
    let wide = EmbeddingSpace::new("local/wide", 3072).unwrap();
    assert_eq!(
        distance_expr(&wide, ""),
        "embedding::halfvec(3072) <=> $1::vector::halfvec(3072)"
    );
}

#[test]
fn a_pre_halfvec_pgvector_is_refused_before_the_ddl() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&base_url());
    let store = schema.open();
    let wide = EmbeddingSpace::new("local/wide", 3072).unwrap();
    let err = store
        .with_client(|client| apply_vector_ddl_with(client, &wide, |_| Ok("0.6.2".to_owned())))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("0.7.0"), "{err}");
}
