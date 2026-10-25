// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing embedding-space state across Postgres bootstrap.
//!
//! ## Owns
//! - Fresh and recorded space reconciliation after bootstrap
//! - Vector validation and halfvec bootstrap gates
//!
//! ## Does not own
//! - Bootstrap lock concurrency
//! - Embedding-space identity policy
//!
//! ## Boundary
//!
//! Changes to Postgres bootstrap space handling should normally require changes here.
//! Changes to embedding-space policy should not require changes here.
//!
//! Keep this module focused on bootstrap space state.

use super::{apply_vector_ddl_with, distance_expr};
use crate::db_url::postgres_url_from_env;
use crate::embedding_space::EmbeddingSpace;
use crate::pg_scoped_schema::ScopedSchema;
use clio_types::ErrorCode;

#[test]
fn current_bootstrap_reopens_the_recorded_space() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&postgres_url_from_env());
    let mut store = schema.open();
    let configured = EmbeddingSpace::new("bootstrap/model", 3072).expect("configured space");
    let adopted = store
        .apply_configured_space(&configured, false)
        .expect("fresh bootstrap adopts its configured space");
    assert!(adopted.switched);
    assert_eq!(adopted.purged_rows, 0);
    drop(store);

    let mut reopened = schema.open();
    assert_eq!(reopened.space(), &configured);
    let unchanged = reopened
        .apply_configured_space(&configured, false)
        .expect("recorded configuration is a no-op");
    assert!(!unchanged.switched);
    assert_eq!(unchanged.from, configured);
    assert_eq!(unchanged.to, configured);
    assert_eq!(unchanged.purged_rows, 0);

    let different = EmbeddingSpace::new("bootstrap/other", 384).expect("different space");
    assert_eq!(
        reopened
            .apply_configured_space(&different, false)
            .expect_err("a current bootstrap is no longer a fresh space")
            .code,
        ErrorCode::ConfigCorrupt
    );
    let switched = reopened
        .apply_configured_space(&different, true)
        .expect("confirmed switch");
    assert!(switched.switched);
    assert_eq!(switched.from, configured);
    assert_eq!(switched.to, different);
    assert_eq!(reopened.space(), &different);
}

#[test]
fn reopened_space_rejects_wrong_vector_identity() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&postgres_url_from_env());
    let mut store = schema.open();
    let configured = EmbeddingSpace::new("bootstrap/model", 768).expect("space");
    store
        .apply_configured_space(&configured, false)
        .expect("adopt configured space");
    drop(store);
    let reopened = schema.open();
    reopened
        .validate_dims(&vec![0.0; 768])
        .expect("active width");
    reopened
        .validate_model("bootstrap/model")
        .expect("active model");
    assert_eq!(
        reopened
            .validate_dims(&[0.0])
            .expect_err("wrong width")
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        reopened
            .validate_model("foreign/model")
            .expect_err("wrong model")
            .code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn halfvec_bootstrap_checks_extension_before_vector_ddl() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&postgres_url_from_env());
    let store = schema.open();
    let wide = EmbeddingSpace::new("bootstrap/wide", 3072).expect("wide space");
    let error = store
        .with_client(|client| apply_vector_ddl_with(client, &wide, |_| Ok("0.6.2".to_owned())))
        .expect_err("old extension refuses halfvec");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(error.message.contains("0.7.0"), "{error}");
    assert!(error.message.contains("upgrade"), "{error}");
    let column = schema
        .query_value("SELECT format_type(atttypid, atttypmod) FROM pg_attribute WHERE attrelid = 'item_embeddings'::regclass AND attname = 'embedding'")
        .expect("column type");
    assert_eq!(column.as_deref(), Some("vector(384)"));

    store
        .test_execute("DROP TABLE item_embeddings")
        .expect("remove derived table");
    store
        .with_client(|client| apply_vector_ddl_with(client, &wide, |_| Ok("0.7.0".to_owned())))
        .expect("supported halfvec DDL");
}

#[test]
fn distance_sql_uses_the_recorded_width_and_alias() {
    let narrow = EmbeddingSpace::new("bootstrap/narrow", 384).expect("plain vector");
    let wide = EmbeddingSpace::new("bootstrap/wide", 3072).expect("halfvec");
    assert_eq!(distance_expr(&narrow, ""), "embedding <=> $1");
    assert_eq!(distance_expr(&narrow, "v"), "v.embedding <=> $1");
    assert_eq!(
        distance_expr(&wide, ""),
        "embedding::halfvec(3072) <=> $1::vector::halfvec(3072)"
    );
    assert_eq!(
        distance_expr(&wide, "v"),
        "v.embedding::halfvec(3072) <=> $1::vector::halfvec(3072)"
    );
}

#[test]
fn a_failed_space_switch_preserves_the_current_bootstrap() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&postgres_url_from_env());
    let mut store = schema.open();
    let original = store.space().clone();
    let requested = EmbeddingSpace::new("bootstrap/refused", 768).expect("requested space");
    schema
        .execute(
            "CREATE FUNCTION refuse_space_write() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'refuse space setting'; END $$;
             CREATE TRIGGER refuse_space_write BEFORE UPDATE ON schema_settings
             FOR EACH ROW WHEN (NEW.key = 'embedding_model')
             EXECUTE FUNCTION refuse_space_write()",
        )
        .expect("inject settings write failure after vector recreation");
    let error = store
        .apply_configured_space(&requested, true)
        .expect_err("space switch must encounter settings write failure");
    assert!(error.message.contains("refuse space setting"), "{error}");
    schema
        .execute("DROP TRIGGER refuse_space_write ON schema_settings; DROP FUNCTION refuse_space_write()")
        .expect("remove settings failure");
    drop(store);
    let mut reopened = schema.open();
    assert_eq!(reopened.space(), &original);
    assert_eq!(
        schema
            .query_value(
                "SELECT value FROM schema_settings WHERE key = 'postgres_bootstrap_version'"
            )
            .expect("completion marker")
            .as_deref(),
        Some(super::PG_BOOTSTRAP_VERSION)
    );
    assert_eq!(
        schema.query_value("SELECT format_type(atttypid, atttypmod) FROM pg_attribute WHERE attrelid = 'item_embeddings'::regclass AND attname = 'embedding'").expect("vector column type").as_deref(),
        Some("vector(384)"),
        "a failed switch must keep the vector schema consistent with its completion marker"
    );
    reopened
        .apply_configured_space(&requested, true)
        .expect("retry switch after removing failure");
    drop(reopened);
    assert_eq!(schema.open().space(), &requested);
}
