// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres open-edge uniqueness invariant
//! tests: duplicate rejection, supersession, close-then-reopen, the v10 to v11
//! upgrade on a previous-schema database, and the named duplicate diagnostic
//! on reopen. Destructive cases run in private temporary schemas so no test
//! DDL or row cleanup ever touches the shared public schema.
//!
//! ## Owns
//! - Postgres cases for the open-edge uniqueness invariant tests
//! - The private temporary-schema helper used by destructive cases here
//!
//! ## Does not own
//! - SQLite cases and shared fixtures (`triple_open_uniq_tests`)
//! - The invariant implementation (`triple_invariant`, `sql/001_core.sql`)
//!
//! ## Boundary
//!
//! Changes to the open-edge constraint should normally update tests here.
//! Changes to unrelated Postgres triple behavior should not require changes here.

use crate::migrate::{SQL_CORE, SQL_VECTORS_POSTGRES};
use crate::postgres::PostgresStore;
use crate::store::Store;
use crate::test_unique::unique;
use crate::sqlite_triple::sqlite_triple_open_uniq_tests::{
    LEGACY_OPEN_INX, OPEN_UNIQ_INDEX, carrier, edge, kms, raw_open_triple_sql,
};
use clio_types::ErrorCode;

fn open_pg() -> Option<PostgresStore> {
    let url = crate::db_url::postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    match PostgresStore::open(&url, kms()) {
        Ok(store) => Some(store),
        Err(err) => {
            eprintln!(
                "{}",
                crate::pg_skip::pg_skip_note("postgres open-edge invariant tests", &err)
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Postgres (shared instance, serialized by PG_TEST_LOCK)
// ---------------------------------------------------------------------------

/// Scalar probe against the store client (index presence, schema version).
fn pg_scalar<T: postgres::types::FromSqlOwned>(store: &PostgresStore, sql: &str) -> T {
    store
        .with_client(|client| {
            let row = client
                .query_one(sql, &[])
                .map_err(|e| clio_types::AmError::new(ErrorCode::Internal, e.to_string()))?;
            Ok(row.get::<usize, T>(0))
        })
        .expect("scalar query")
}

/// Index probe restricted to the current schema: the shared public schema also
/// holds `triples_open_uniq`, so an unqualified `pg_indexes` count would see
/// it even from a private schema.
fn pg_index_present(store: &PostgresStore, name: &str) -> bool {
    pg_scalar::<i64>(
        store,
        &format!(
            "SELECT COUNT(*)::bigint FROM pg_indexes \
             WHERE indexname = '{name}' AND schemaname = current_schema()"
        ),
    ) > 0
}

/// A private temporary schema on the shared Compose database.
///
/// Destructive tests reach it through an `options` URL that pins `search_path`
/// to the schema (then `public`, so the pgvector `vector` type and hnsw op
/// classes still resolve). Unqualified DDL and DML land in the private schema,
/// so the shared public schema never loses its unique index and never holds
/// test rows (control: no destructive DDL on public). The `DROP INDEX IF
/// EXISTS` statements here only run when the private schema already owns the
/// named index, so the search-path fall-through can never reach `public`.
/// The schema drops itself with CASCADE when the guard goes out of scope,
/// even on panic.
struct PrivateSchema {
    url: String,
    options_url: String,
    name: String,
}

impl PrivateSchema {
    /// Create the schema; `None` skips the test when the database is down.
    fn create(prefix: &str) -> Option<Self> {
        let url = crate::db_url::postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
        let name = unique(prefix).replace('-', "_");
        let mut client = match crate::pg_tls::connect(url.as_str()) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("skipping postgres open-edge invariant tests: cannot connect: {e}");
                return None;
            }
        };
        client
            .batch_execute(&format!("CREATE SCHEMA {name}"))
            .expect("create private schema");
        let sep = if url.contains('?') { '&' } else { '?' };
        Some(Self {
            options_url: format!("{url}{sep}options=-c%20search_path%3D{name}%2Cpublic"),
            url,
            name,
        })
    }

    /// Apply the full current schema DDL inside the private schema.
    fn bootstrap(&self) {
        let ddl = format!("{SQL_CORE}\n{SQL_VECTORS_POSTGRES}");
        self.client()
            .batch_execute(&ddl)
            .expect("bootstrap private schema");
    }

    /// Reshape the private schema to the previous (v10) revision.
    fn shape_v10(&self) {
        self.client()
            .batch_execute(&format!(
                "DROP INDEX IF EXISTS {OPEN_UNIQ_INDEX}; \
                 CREATE INDEX IF NOT EXISTS triples_bank_id_subject_predicate_open_both_inx \
                 ON triples (bank_id, subject, predicate) \
                 WHERE valid_until IS NULL AND tx_until IS NULL; \
                 UPDATE schema_settings SET value = '10' WHERE key = 'schema_version';"
            ))
            .expect("shape private schema to v10");
    }

    /// Raw client whose unqualified names resolve to the private schema.
    fn client(&self) -> postgres::Client {
        crate::pg_tls::connect(&self.options_url).expect("connect to private schema")
    }
}

impl Drop for PrivateSchema {
    fn drop(&mut self) {
        if let Ok(mut client) = crate::pg_tls::connect(self.url.as_str()) {
            let _ = client.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.name));
        }
    }
}

/// Raw legacy rows: one items row and two open triples for the same
/// (bank_id, subject, predicate), inserted unqualified into the private schema.
fn seed_legacy_duplicate(
    client: &mut postgres::Client,
    bank: &str,
    item: &str,
    e1: &str,
    dup: &str,
) {
    let sql = format!(
        "INSERT INTO items (id, bank_id, kind, episodic_type, epistemic_kind, \
         valid_from, tx_from, created_at, updated_at) VALUES \
         ('{item}', '{bank}', 'episodic', 'triple', 'fact', '2024-01-01', \
         '2024-01-01', '2024-01-01', '2024-01-01'); \
         INSERT INTO triples (id, bank_id, item_id, subject, predicate, object_ciphertext, \
         epistemic_kind, valid_from, tx_from, created_at) VALUES \
         ('{e1}', '{bank}', '{item}', 'Alice', 'lives_in', 'sealed-fixture', 'fact', \
         '2024-01-01', '2024-01-01', '2024-01-01'), \
         ('{dup}', '{bank}', '{item}', 'Alice', 'lives_in', 'sealed-fixture', \
         'fact', '2024-01-01', '2024-01-01', '2024-01-01');"
    );
    client
        .batch_execute(&sql)
        .expect("seed legacy duplicate rows");
}

#[test]
fn pg_duplicate_open_edge_rejected_and_supersede_succeeds() {
    let _guard = crate::pg_lock::lock_pg_test();
    let Some(pg) = open_pg() else {
        return;
    };
    let bank = unique("pg-dup");
    let (i1, i2, e1, e2) = (unique("i1"), unique("i2"), unique("e1"), unique("e2"));
    pg.commit_triple_add(
        &carrier(&i1, &bank, "2024-01-01", "2024-01-01", "Chicago"),
        &edge(
            &e1,
            &bank,
            &i1,
            "lives_in",
            "Chicago",
            "2024-01-01",
            "2024-01-01",
        ),
        false,
        "agent",
    )
    .unwrap();
    let err = pg
        .commit_triple_add(
            &carrier(&i2, &bank, "2024-02-01", "2024-02-01", "Denver"),
            &edge(
                &e2,
                &bank,
                &i2,
                "lives_in",
                "Denver",
                "2024-02-01",
                "2024-02-01",
            ),
            false,
            "agent",
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let out = pg
        .commit_triple_add(
            &carrier(&i2, &bank, "2024-02-01", "2024-02-01", "Denver"),
            &edge(
                &e2,
                &bank,
                &i2,
                "lives_in",
                "Denver",
                "2024-02-01",
                "2024-02-01",
            ),
            true,
            "agent",
        )
        .unwrap();
    assert_eq!(out.prior_edge_ids, vec![e1.clone()]);
    let prior = pg.get_triple(&bank, &e1).unwrap().unwrap();
    assert_eq!(prior.valid_until.as_deref(), Some("2024-02-01"));
    assert_eq!(prior.tx_until.as_deref(), Some("2024-02-01"));
}

#[test]
fn pg_close_then_reopen_and_different_predicate_allowed() {
    let _guard = crate::pg_lock::lock_pg_test();
    let Some(pg) = open_pg() else {
        return;
    };
    let bank = unique("pg-reopen");
    let (i1, i2, i3, e1, e2, e3) = (
        unique("i1"),
        unique("i2"),
        unique("i3"),
        unique("e1"),
        unique("e2"),
        unique("e3"),
    );
    pg.commit_triple_add(
        &carrier(&i1, &bank, "2024-01-01", "2024-01-01", "Chicago"),
        &edge(
            &e1,
            &bank,
            &i1,
            "lives_in",
            "Chicago",
            "2024-01-01",
            "2024-01-01",
        ),
        false,
        "agent",
    )
    .unwrap();
    pg.triple_end(
        &bank,
        "Alice",
        "lives_in",
        Some("Chicago"),
        "2024-06-01",
        "2024-06-01",
    )
    .unwrap();
    pg.commit_triple_add(
        &carrier(&i2, &bank, "2024-07-01", "2024-07-01", "Denver"),
        &edge(
            &e2,
            &bank,
            &i2,
            "lives_in",
            "Denver",
            "2024-07-01",
            "2024-07-01",
        ),
        false,
        "agent",
    )
    .unwrap();
    pg.commit_triple_add(
        &carrier(&i3, &bank, "2024-07-01", "2024-07-01", "Boston"),
        &edge(
            &e3,
            &bank,
            &i3,
            "works_in",
            "Boston",
            "2024-07-01",
            "2024-07-01",
        ),
        false,
        "agent",
    )
    .unwrap();
}

#[test]
fn pg_existing_db_gains_unique_index_on_reopen() {
    let _guard = crate::pg_lock::lock_pg_test();
    let Some(schema) = PrivateSchema::create("am_openuniq_up") else {
        return;
    };
    schema.bootstrap();
    // Build a database with the previous schema shape: old non-unique open
    // index, no unique index, schema_version '10'.
    schema.shape_v10();
    let pg = PostgresStore::open(&schema.options_url, kms()).expect("open v10 database");
    // The open converges the shape: it drops the legacy index, builds the
    // unique one, and stamps the current schema version.
    assert!(!pg_index_present(&pg, LEGACY_OPEN_INX));
    assert!(pg_index_present(&pg, OPEN_UNIQ_INDEX));
    let version = pg_scalar::<String>(
        &pg,
        "SELECT value FROM schema_settings WHERE key = 'schema_version'",
    );
    assert_eq!(version, "1");
    let bank = unique("pg-up");
    let (i1, e1) = (unique("i1"), unique("e1"));
    pg.commit_triple_add(
        &carrier(&i1, &bank, "2024-01-01", "2024-01-01", "Chicago"),
        &edge(
            &e1,
            &bank,
            &i1,
            "lives_in",
            "Chicago",
            "2024-01-01",
            "2024-01-01",
        ),
        false,
        "agent",
    )
    .unwrap();
    // On the upgraded schema the unique index rejects a raw duplicate insert.
    let dup = unique("dup");
    assert!(
        pg.test_execute(&raw_open_triple_sql(&bank, &i1, &dup))
            .is_err()
    );
}

#[test]
fn pg_legacy_duplicate_open_edges_fail_with_named_diagnostic() {
    let _guard = crate::pg_lock::lock_pg_test();
    let Some(schema) = PrivateSchema::create("am_openuniq_dup") else {
        return;
    };
    schema.bootstrap();
    let bank = unique("pg-legacy");
    let (item, e1, dup) = (unique("i1"), unique("e1"), unique("dup"));
    {
        let mut client = schema.client();
        client
            .batch_execute(&format!("DROP INDEX IF EXISTS {OPEN_UNIQ_INDEX};"))
            .expect("drop unique index in private schema");
        seed_legacy_duplicate(&mut client, &bank, &item, &e1, &dup);
    }
    let Err(err) = PostgresStore::open(&schema.options_url, kms()) else {
        panic!("legacy duplicate must fail store open");
    };
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    let msg = err.to_string();
    assert!(msg.contains("duplicate open triples"), "{msg}");
    assert!(msg.contains("operator-confirmed"), "{msg}");
    // Operator-confirmed repair: close the extra open edge (never DELETE).
    schema
        .client()
        .execute(
            "UPDATE triples SET valid_until = '2024-02-01', tx_until = '2024-02-01' \
             WHERE id = $1 AND bank_id = $2",
            &[&dup, &bank],
        )
        .expect("close duplicate");
    let pg = PostgresStore::open(&schema.options_url, kms()).expect("reopen after repair");
    assert!(pg_index_present(&pg, OPEN_UNIQ_INDEX));
}
