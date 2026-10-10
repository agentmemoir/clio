// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres multi-width index calibration:
//! the width → index-mode `EXPLAIN` matrix, measured `halfvec` recall against
//! exact search, and the pgvector version gate that guards the `halfvec` path.
//!
//! ## Owns
//! - The width → index-mode `EXPLAIN` matrix (384/768/1536/3072/4096)
//! - Recall of the halfvec ANN path against exact search at 3072 dims
//! - The injected-version refusal test for the halfvec bootstrap gate
//!
//! ## Does not own
//! - Space bootstrap/switch behavior (see `pg_space_tests`)
//! - Width policy (`embedding_space`) or DDL text (`vector_ddl`)
//!
//! ## Boundary
//!
//! Changes to the width/index thresholds or the halfvec-version floor should
//! normally update the calibration here. Changes to space reconciliation should not.
//!
//! Kept separate from `pg_space_tests` so each module stays within the source-size
//! limit. Every case uses a private schema that is dropped on exit.

use crate::embedding_space::EmbeddingSpace;
use crate::embedding_space::embedding_space_pg_space_tests::{base_url, item, pseudo_vector, slot};
use crate::pg_scoped_schema::ScopedSchema;
use crate::postgres::{PostgresStore, distance_expr};
use crate::store::Store;
use postgres::Client;

const CALIB_ROWS: usize = 200;
const CALIB_WIDTHS: [usize; 5] = [384, 768, 1536, 3072, 4096];

/// Populate `rows` visible items with vectors at the store's active width.
fn populate(store: &PostgresStore, bank: &str, rows: usize) {
    let dims = store.embedding_space().unwrap().dims();
    let model = store.embedding_space().unwrap().model_id().to_owned();
    for i in 0..rows {
        let id = format!("{bank}-i{i}");
        store.create_memory_item(&item(&id, bank), "actor").unwrap();
        store
            .upsert_vector(&slot(&id, bank, &model, pseudo_vector(i, dims)))
            .unwrap();
    }
}

/// `EXPLAIN` plan lines for a KNN query under the store's active space.
fn explain(store: &PostgresStore, seqscan_off: bool) -> String {
    let space = store.embedding_space().unwrap();
    let query = pseudo_vector(9_999, space.dims);
    let distance = distance_expr(&space, "");
    let sql = format!("EXPLAIN SELECT item_id FROM item_embeddings ORDER BY {distance} LIMIT 5");
    store
        .with_client(|client| {
            if seqscan_off {
                client.batch_execute("SET enable_seqscan = off").unwrap();
            } else {
                client.batch_execute("RESET enable_seqscan").unwrap();
            }
            let embedding = pgvector::Vector::from(query.clone());
            let rows = client.query(&sql, &[&embedding]).unwrap();
            let plan: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
            if seqscan_off {
                client.batch_execute("RESET enable_seqscan").unwrap();
            }
            Ok::<_, clio_types::AmError>(plan.join("\n"))
        })
        .unwrap()
}

/// Index definitions on `item_embeddings` in the store's schema.
fn index_defs(store: &PostgresStore) -> Vec<String> {
    store
        .with_client(|client| {
            let rows = client
                .query(
                    "SELECT indexdef FROM pg_indexes
                     WHERE tablename = 'item_embeddings' AND schemaname = current_schema()",
                    &[],
                )
                .unwrap();
            Ok::<_, clio_types::AmError>(
                rows.iter()
                    .map(|r| r.get::<_, String>(0))
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap()
}

#[test]
fn postgres_width_index_mode_matrix_is_measured() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&base_url());
    let mut store = schema.open();
    let version: String = store
        .with_client(|client| {
            let row = client
                .query_one(
                    "SELECT extversion FROM pg_extension WHERE extname = 'vector'",
                    &[],
                )
                .unwrap();
            Ok::<_, clio_types::AmError>(row.get(0))
        })
        .unwrap();
    eprintln!("calibration: pgvector extversion = {version}");
    assert!(
        crate::pgvector_supports_halfvec(&version),
        "halfvec needs pgvector >= 0.7.0, found {version}"
    );

    for width in CALIB_WIDTHS {
        let space = EmbeddingSpace::new("calibration/model", width).unwrap();
        store.apply_configured_space(&space, true).unwrap();
        populate(&store, &format!("b{width}"), CALIB_ROWS);

        let defs = index_defs(&store);
        let has_hnsw = defs.iter().any(|d| d.contains("hnsw"));
        let plain = explain(&store, false);
        let forced = explain(&store, true);
        let brief = |plan: &str| {
            plan.lines()
                .filter(|l| l.contains("Scan") || l.contains("Limit"))
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        eprintln!(
            "calibration: width={width} mode={} hnsw={has_hnsw} plain=[{}] seqscan_off=[{}]",
            space.index_mode().as_str(),
            brief(&plain),
            brief(&forced)
        );

        if width <= crate::ANN_HALFVEC_MAX_DIMS {
            assert!(has_hnsw, "width {width} must have an ANN index: {defs:?}");
            assert!(
                forced.contains("Index Scan using item_embeddings_embedding_hnsw_inx"),
                "width {width} must serve the cast query through its ANN index:\n{forced}"
            );
            if width > crate::ANN_VECTOR_MAX_DIMS {
                assert!(
                    defs.iter().any(|d| d.contains("halfvec")),
                    "width {width} must use the halfvec expression index: {defs:?}"
                );
            }
        } else {
            assert!(
                !has_hnsw,
                "width {width} must not create an ANN index: {defs:?}"
            );
            assert!(
                forced.contains("Seq Scan"),
                "width {width} must fall back to an exact scan:\n{forced}"
            );
        }
    }
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn postgres_halfvec_recall_at_3072_measures_the_ann_index() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&base_url());
    let mut store = schema.open();
    let space = EmbeddingSpace::new("calibration/wide", 3072).unwrap();
    store.apply_configured_space(&space, true).unwrap();
    populate(&store, "recall", CALIB_ROWS);

    // Guard that the "indexed" run really exercises the HNSW expression index;
    // a sequential scan here would make the recall below vacuous.
    let forced = explain(&store, true);
    assert!(
        forced.contains("Index Scan using item_embeddings_embedding_hnsw_inx"),
        "the indexed recall run must use the ANN index:\n{forced}"
    );

    let k = 10_i64;
    let mut overlap = 0_usize;
    let mut total = 0_usize;
    for q in 0..10 {
        let query = pseudo_vector(50_000 + q, 3072);
        let embedding = pgvector::Vector::from(query.clone());
        let run = |client: &mut Client| -> Vec<String> {
            let rows = client
                .query(
                    "SELECT item_id FROM item_embeddings
                     ORDER BY embedding::halfvec(3072) <=> $1::vector::halfvec(3072)
                     LIMIT $2",
                    &[&embedding, &k],
                )
                .unwrap();
            rows.iter().map(|r| r.get::<_, String>(0)).collect()
        };
        // ANN run: force the index so the result is approximate, not exact.
        let indexed = store
            .with_client(|client| {
                client.batch_execute("SET enable_seqscan = off").unwrap();
                let rows = run(client);
                client.batch_execute("RESET enable_seqscan").unwrap();
                Ok::<_, clio_types::AmError>(rows)
            })
            .unwrap();
        // Exact run: the same query with index scans disabled.
        let exact = store
            .with_client(|client| {
                client.batch_execute("SET enable_indexscan = off").unwrap();
                let rows = run(client);
                client.batch_execute("RESET enable_indexscan").unwrap();
                Ok::<_, clio_types::AmError>(rows)
            })
            .unwrap();
        overlap += indexed.iter().filter(|id| exact.contains(id)).count();
        total += exact.len();
    }
    let recall = overlap as f64 / total as f64;
    eprintln!("calibration: halfvec recall@10 at 3072 dims = {recall:.4} ({overlap}/{total})");
    assert!(
        recall >= 0.9,
        "halfvec recall@10 at 3072 dims must stay >= 0.9, measured {recall:.4}"
    );
}

#[test]
fn pgvector_below_halfvec_floor_fails_closed_with_guidance() {
    let err = crate::postgres::halfvec_unsupported("0.6.2");
    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("0.7.0"), "{err}");
    assert!(err.message.contains("upgrade"), "{err}");
    assert!(!crate::pgvector_supports_halfvec("0.6.2"));
}

#[test]
fn old_pgvector_bootstrap_refuses_halfvec_before_ddl() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&base_url());
    let store = schema.open();
    let space = EmbeddingSpace::new("calibration/wide", 3072).unwrap();
    // Drive the exact bootstrap gate that `apply_vector_ddl` uses, with an
    // injected pre-0.7.0 version, so the refusal path is exercised end to end
    // without a real old pgvector install.
    let err = store
        .with_client(|client| {
            crate::postgres::apply_vector_ddl_with(client, &space, |_| Ok("0.6.2".to_owned()))
        })
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("0.7.0"), "{err}");
    assert!(err.message.contains("upgrade"), "{err}");
}
