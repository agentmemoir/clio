// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests `clio backup restore` against Postgres stores: the
//! acceptance path into a second empty scoped schema, the backend and
//! table-set refusals, the dry run, and the count-mismatch report.
//!
//! ## Owns
//! - The Postgres-backed restore behavior, refusal, and report assertions
//!
//! ## Does not own
//! - The restore flow (see `backup_restore_flow` and `backup_restore_cli`)
//! - The one-transaction restore (see `clio-store`'s postgres restore)
//!
//! ## Boundary
//!
//! Test-only, and every test serializes on the shared Postgres test lock.
//! Never compiled into production builds.

use super::assemble_tests::assemble_pg_archive;
use crate::backup_cli_fixtures::{Scratch, item, run_json_in_with};
use clio_store::{DekProvider, LocalDevKms, PostgresStore, ScopedSchema, Store, lock_pg_test};
use serde_json::json;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

/// The shared test database's URL: the gate runs every test with `CLIO_DATABASE_URL`.
pub(crate) fn base_url() -> String {
    std::env::var("CLIO_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://clio:clio@127.0.0.1:34310/clio".to_owned())
}

/// Seed a scoped schema's store with `items` items over a persistent key store
/// at `kms_path`.
///
/// The store handle is dropped; the tables and rows stay in the schema.
pub(crate) fn seed(scoped: &ScopedSchema, kms_path: &Path, items: usize) {
    // The persistent key store seals its slots under the Clio encryption key.
    crate::setup_support_tests::ensure_test_key();
    let kms: Arc<dyn DekProvider> =
        Arc::new(LocalDevKms::open_persistent(kms_path).expect("open the key store"));
    let store = PostgresStore::open(scoped.url(), kms).expect("open the postgres store");
    for index in 0..items {
        let id = format!("item-{index:04}");
        store
            .create_memory_item(&item(&id, "PLANTED-SECRET-abc123"), "harness")
            .expect("write an item");
    }
}

/// Snapshot `scoped`'s store through the real `clio backup create`, carrying
/// the key store at `kms_path`, and return the snapshot id.
pub(crate) fn snapshot(scratch: &Scratch, scoped: &ScopedSchema, kms_path_string: &str) -> String {
    let (code, payload, err) = run_json_in_with(
        scratch,
        &["create"],
        &[
            ("CLIO_DATABASE_URL", scoped.url()),
            ("CLIO_KMS_FILE", kms_path_string),
        ],
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    payload["snapshot_id"]
        .as_str()
        .expect("the create produced a snapshot id")
        .to_owned()
}

/// An orphan insert into the schema's vector table must be refused, and a
/// legitimate insert must succeed.
///
/// The restore disables every loaded table's triggers, including the foreign
/// keys, inside its one transaction and re-enables them before the commit.
/// The assertion is behavioral, not message-matching: the store's error
/// mapping reports the driver's terse "db error" by design, so a real item id
/// first proves the load shape is right, then an orphan proves enforcement
/// returned. The vector literal is built at the table's declared width, read
/// from the catalog through the harness's own query helper.
fn orphan_insert_refused(target: &ScopedSchema, schema: &str) {
    let dims: i32 = target
        .query_value(&format!(
            "SELECT a.atttypmod::text \
             FROM pg_catalog.pg_attribute a \
             JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = '{schema}' AND c.relname = 'item_embeddings' \
               AND a.attname = 'embedding'"
        ))
        .expect("read the vector column's width")
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(0);
    let dims = usize::try_from(dims).unwrap_or(0);
    let dims = if dims == 0 { 8 } else { dims };
    let vector = format!("[{}]", vec!["0.1"; dims].join(","));
    let real_item = target
        .query_value(&format!(
            "SELECT id::text FROM \"{schema}\".items ORDER BY id LIMIT 1"
        ))
        .expect("read one restored item id")
        .expect("the restored store has an item");
    // A legitimate row loads: the shape, the width, and the row id are right.
    target
        .execute(&format!(
            "INSERT INTO \"{schema}\".item_embeddings \
             (item_id, bank_id, model_id, dims, embedding, created_at, updated_at) \
             VALUES ('{real_item}', 'demo', 'm', {dims}, '{vector}'::vector, 't0', 't0')"
        ))
        .expect("a real item's embedding insert succeeds after the restore");
    // An orphan is refused: enforcement is back.
    let error = target
        .execute(&format!(
            "INSERT INTO \"{schema}\".item_embeddings \
             (item_id, bank_id, model_id, dims, embedding, created_at, updated_at) \
             VALUES ('orphan-item', 'demo', 'm', {dims}, '{vector}'::vector, 't0', 't0')"
        ))
        .expect_err("the foreign key must refuse an orphan insert after the restore");
    let _ = error;
}

#[test]
fn a_postgres_archive_restores_into_a_second_empty_scoped_schema() {
    let _guard = lock_pg_test();
    let base = base_url();
    let a = ScopedSchema::new(&base);
    let b = ScopedSchema::new(&base);
    let scratch = Scratch::new("restore-pg-ok");
    let kms_a = scratch.join("kms-a.json").display().to_string();
    let kms_b = scratch.join("kms-b.json").display().to_string();
    seed(&a, Path::new(&kms_a), 2);
    seed(&b, Path::new(&kms_b), 0);

    let id = snapshot(&scratch, &a, &kms_a);
    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["restore", &id, "--yes"],
        &[("CLIO_DATABASE_URL", b.url()), ("CLIO_KMS_FILE", &kms_b)],
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["result"], "restored", "{payload}");
    assert_eq!(payload["counts"]["after"]["items"], 2, "{payload}");
    assert_eq!(payload["counts"]["after"]["triples"], 0, "{payload}");

    // The rows actually landed in B, and A kept its own.
    let state_b = clio_store::postgres_target_state(b.url()).expect("measure B");
    assert_eq!(
        state_b.count_of("items"),
        Some(2),
        "B holds the restored rows"
    );
    assert_eq!(state_b.count_of("triples"), Some(0));
    let state_a = clio_store::postgres_target_state(a.url()).expect("measure A");
    assert_eq!(state_a.count_of("items"), Some(2), "A is untouched");

    // The foreign keys came back with the triggers: an orphan insert into B's
    // vector table is refused again.
    orphan_insert_refused(&b, &state_b.schema);
}

#[test]
fn a_target_with_an_extra_table_is_refused_naming_both_sets() {
    let _guard = lock_pg_test();
    let base = base_url();
    let a = ScopedSchema::new(&base);
    let b = ScopedSchema::new(&base);
    let scratch = Scratch::new("restore-pg-tables");
    let kms_a = scratch.join("kms-a.json").display().to_string();
    seed(&a, Path::new(&kms_a), 1);
    seed(&b, &scratch.join("kms-b.json"), 1);

    let id = snapshot(&scratch, &a, &kms_a);

    // B grows a table the archive does not carry.
    let schema = clio_store::postgres_target_state(b.url())
        .expect("measure B")
        .schema;
    b.execute(&format!(
        "CREATE TABLE \"{schema}\".extra_table (id text primary key)"
    ))
    .expect("add a table to B");

    let (code, _, err) = run_json_in_with(
        &scratch,
        &["restore", &id, "--yes", "--allow-missing-keys"],
        &[("CLIO_DATABASE_URL", b.url())],
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("extra_table"), "{err}");
    assert!(err.contains("archive"), "{err}");
    assert!(err.contains("target"), "{err}");
    let state = clio_store::postgres_target_state(b.url()).expect("measure B");
    assert_eq!(state.count_of("items"), Some(1), "B's rows are intact");
}

#[test]
fn an_archive_whose_counts_overshoot_the_load_is_reported_as_a_count_mismatch() {
    let _guard = lock_pg_test();
    let base = base_url();
    let a = ScopedSchema::new(&base);
    let b = ScopedSchema::new(&base);
    let scratch = Scratch::new("restore-pg-counts");
    let kms_a = scratch.join("kms-a.json").display().to_string();
    seed(&a, Path::new(&kms_a), 2);
    // B holds one row, so the restore takes a pre-restore snapshot first.
    seed(&b, &scratch.join("kms-b.json"), 1);

    // A real dump of A, then an archive whose meta claims one more items row
    // than its member can load. A trigger cannot play this role: the restore
    // disables every loaded table's triggers, user ones included, for its
    // whole transaction, so nothing can eat rows during the load; the count
    // check is the defense against an archive that misreports itself.
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let mut staged_paths: Vec<std::path::PathBuf> = Vec::new();
    let mut stage = |_t: &clio_store::PgTable,
                     _ordinal: usize|
     -> Result<(std::path::PathBuf, std::fs::File), clio_types::AmError> {
        let (path, file) = clio_backup::staging::begin_staged(&backup_dir, "dump-table")?;
        staged_paths.push(path.clone());
        Ok((path, file))
    };
    let mut facts = clio_store::snapshot_postgres_into(a.url(), &mut stage).expect("dump A");
    for table in &mut facts.tables {
        if table.table == "items" {
            table.rows += 1;
        }
    }
    let id = assemble_pg_archive(&backup_dir, &facts, &staged_paths, None, false);
    drop(staged_paths);

    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["restore", &id, "--yes", "--allow-missing-keys"],
        &[("CLIO_DATABASE_URL", b.url())],
    );
    assert_eq!(code, ExitCode::from(3), "{err}");
    assert_eq!(payload["result"], "count-mismatch", "{payload}");
    // A count-mismatched restore is not a successful one. A script that checks
    // only `ok` must not be told this restore worked.
    assert_eq!(payload["ok"], json!(false), "{payload}");
    assert!(payload["failure"].is_string(), "{payload}");
    assert!(
        payload["failure"]
            .as_str()
            .unwrap_or_default()
            .contains("items"),
        "the differing table is named: {payload}"
    );
    assert!(
        payload["pre_restore_snapshot"]["id"].is_string(),
        "the way back is named in the report: {payload}"
    );
    // The load itself happened: B holds the archive's real rows.
    let state_b = clio_store::postgres_target_state(b.url()).expect("measure B");
    assert_eq!(state_b.count_of("items"), Some(2), "{state_b:?}");
}

#[test]
fn a_dry_run_reports_the_plan_and_writes_nothing() {
    let _guard = lock_pg_test();
    let base = base_url();
    let a = ScopedSchema::new(&base);
    let b = ScopedSchema::new(&base);
    let scratch = Scratch::new("restore-pg-dry");
    let kms_a = scratch.join("kms-a.json").display().to_string();
    seed(&a, Path::new(&kms_a), 2);
    seed(&b, &scratch.join("kms-b.json"), 0);
    let before = clio_store::postgres_target_state(b.url()).expect("measure B before");

    let id = snapshot(&scratch, &a, &kms_a);
    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["restore", &id, "--yes", "--allow-missing-keys", "--dry-run"],
        &[("CLIO_DATABASE_URL", b.url())],
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["result"], "planned", "{payload}");
    assert_eq!(payload["dry_run"], true, "{payload}");
    assert_eq!(
        payload["counts"]["before"]["items"].as_u64(),
        before.count_of("items"),
        "{payload}"
    );
    assert_eq!(
        payload["counts"]["after"],
        serde_json::Value::Null,
        "{payload}"
    );
    assert_eq!(
        payload["pre_restore_snapshot"]["skipped"], true,
        "{payload}"
    );
    assert_eq!(
        payload["pre_restore_snapshot"]["reason"],
        "the target store is empty, so there is nothing to preserve",
        "{payload}"
    );

    let after = clio_store::postgres_target_state(b.url()).expect("measure B after");
    assert_eq!(after, before, "a dry run writes nothing");
    // Only the original archive is in the backup directory: the staged dump
    // members and the untouched plan are gone with it.
    let entries: Vec<String> = std::fs::read_dir(scratch.join("backups"))
        .expect("read the backup directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
}
