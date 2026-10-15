// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the dump's edge on schemas that are not clio stores: a
//! schema with tables but no `items` table dumps and reports no banks.
//!
//! ## Owns
//! - The no-items dump assertion
//!
//! ## Does not own
//! - The dump's store-schema behavior (see `postgres_dump_tests`)
//! - The discovery classification (see `postgres_discovery_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::pg_lock::lock_pg_test;
use crate::pg_scoped_schema::ScopedSchema;
use crate::postgres_backup_model::PgTable;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// A staging directory, removed when the guard drops.
struct StageDir(PathBuf);

impl StageDir {
    /// The directory path.
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StageDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh staging directory, unique per call within and across processes.
fn stage_dir(tag: &str) -> StageDir {
    let dir = std::env::temp_dir().join(format!(
        "clio-pg-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir_all(&dir).expect("create the stage dir");
    StageDir(dir)
}

/// Run one SQL batch on a fresh connection.
fn batch(url: &str, sql: &str) {
    let mut client = crate::pg_tls::connect(url).expect("batch connect");
    client.batch_execute(sql).expect("run the batch");
}

/// The stage sink: one file per table, each handed path recorded.
fn tracking_stage(
    dir: &Path,
    handed: Rc<RefCell<Vec<PathBuf>>>,
) -> impl FnMut(&PgTable, usize) -> Result<(PathBuf, std::fs::File), clio_types::AmError> {
    move |_table, _ordinal| {
        let path = dir.join(format!(
            "member-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let file = std::fs::File::create(&path).map_err(|e| {
            clio_types::AmError::new(clio_types::ErrorCode::Internal, format!("{e}"))
        })?;
        handed.borrow_mut().push(path.clone());
        Ok((path, file))
    }
}

/// A schema that holds tables but no store: the dump copies what is there and
/// reports no banks. This pins the banks read's no-items arm and keeps a
/// non-store schema from being refused for the wrong reason.
#[test]
fn a_schema_without_an_items_table_dumps_and_reports_no_banks() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&crate::db_url::postgres_url_from_env());
    batch(
        scoped.url(),
        "CREATE TABLE probe (id text primary key, note text);",
    );
    let dir = stage_dir("dump-no-items");
    let handed = Rc::new(RefCell::new(Vec::new()));

    let facts = crate::snapshot_postgres_into(
        scoped.url(),
        &mut tracking_stage(dir.path(), Rc::clone(&handed)),
    )
    .expect("dump a plain schema");

    // The one table was dumped and the banks list is empty: there is no
    // `items` table to read banks from.
    assert_eq!(facts.tables.len(), 1);
    assert_eq!(facts.tables[0].table, "probe");
    assert!(facts.banks.is_empty(), "{:?}", facts.banks);
    assert_eq!(facts.tables[0].rows, 0);
}
