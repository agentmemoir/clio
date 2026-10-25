// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests what one Postgres restore report's `rows_loaded` means:
//! the rows the restore transaction counted for itself, never the
//! archive's own arithmetic. It restores an archive whose declared per-table
//! counts exceed the rows its members carry, so the declared total and the
//! observed total differ, and pins which of the two the field reports.
//!
//! ## Owns
//! - The declared-count-differing restore report assertions
//!
//! ## Does not own
//! - The restore transaction's order, refusals, or rollback
//!   (see `postgres_restore_tests`)
//! - The target measurement on schemas that are not clio stores
//!   (see `postgres_restore_state_tests`)
//! - The staged-dump harness and shared helpers (see `postgres_restore_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the restore report's `rows_loaded` changes meaning;
//! they never change that meaning. Never compiled into production builds.
//!
//! Keep this module focused on the restore report's row total.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::pg_scoped_schema::ScopedSchema;
use crate::postgres_restore::{
    PostgresRestoreReport, PostgresRestoreRequest, RestoreTable, restore_postgres,
};
use crate::store::Store;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// The declared count no real member can reach: far above the two rows this
/// module's archive carries.
const INFLATED: u64 = 1_000_000_000;

/// A name unique per process and instant, for the scratch directory.
fn unique(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{tag}_{}_{}", std::process::id(), nanos)
}

/// A scratch directory for staged members, removed on drop.
struct ScratchDir {
    path: PathBuf,
    next: std::cell::Cell<usize>,
}

impl ScratchDir {
    /// A fresh, empty scratch directory.
    fn new() -> Self {
        let path = std::env::temp_dir().join(unique("clio-restore-rows"));
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Self {
            path,
            next: std::cell::Cell::new(0),
        }
    }

    /// The path the next dumped table member streams into.
    fn new_member(&self) -> PathBuf {
        let ordinal = self.next.get();
        self.next.set(ordinal + 1);
        self.path.join(format!("member_{ordinal:03}.txt"))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The report's total, summed from its own per-table observations.
fn observed_total(report: &PostgresRestoreReport) -> u64 {
    report.table_counts.iter().map(|(_, count)| count).sum()
}

/// One memory item, for the source store this test dumps.
fn item(id: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: "b-1".into(),
        subject_id: format!("subj-{id}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.5),
        source_ref: Some("turn_1".into()),
        snapshot: Some(json!({"service": "deploy-bot"})),
        gist: Some(format!("content for {id}")),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// An archive whose declared counts exceed the rows its members carry: the
/// report's `rows_loaded` is the observation, not the declaration.
///
/// The declared total is far above anything a load could produce, so a
/// `rows_loaded` that still matched it would be the archive's arithmetic, and
/// the count a full load really observes is pinned exactly.
#[test]
fn rows_loaded_reports_the_observed_rows_not_the_declared_ones() {
    let _guard = lock_pg_test();
    let base = postgres_url_from_env();
    // A real dump of a store holding exactly two items, so the members carry
    // the target's true column set and the load cannot fail on a hand-written
    // row. The archive's declared count is then inflated to a value no load
    // could produce, so the declared total and the observed total differ.
    let source = ScopedSchema::new(&base);
    {
        let store = source.open();
        for id in ["i-1", "i-2"] {
            store
                .create_memory_item(&item(id), "harness")
                .expect("seed the source item");
        }
    }
    let target = ScopedSchema::new(&base);
    target.open();
    let mut probe = crate::pg_tls::connect(target.url()).expect("schema probe connect");
    let schema: String = probe
        .query_one("SELECT current_schema()", &[])
        .expect("read current_schema")
        .get::<_, Option<String>>(0)
        .expect("the scoped search_path resolves to a schema");

    let dir = ScratchDir::new();
    let mut staged: Vec<PathBuf> = Vec::new();
    let mut stage = |_table: &crate::postgres_backup_model::PgTable,
                     _ordinal: usize|
     -> Result<(PathBuf, std::fs::File), clio_types::AmError> {
        let path = dir.new_member();
        let file = std::fs::File::create(&path).expect("create the member file");
        staged.push(path.clone());
        Ok((path, file))
    };
    let facts = crate::postgres_dump::snapshot_postgres_into(source.url(), &mut stage)
        .expect("dump the source store");

    // The declared count for `items` is inflated; every other table keeps the
    // dump's own count, which is already what a load will observe.
    let tables: Vec<RestoreTable> = facts
        .tables
        .iter()
        .zip(&staged)
        .map(|(table, path)| RestoreTable {
            table: table.table.clone(),
            columns: table
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect(),
            member_path: path.clone(),
            rows: if table.table == "items" {
                INFLATED
            } else {
                table.rows
            },
        })
        .collect();
    let request = PostgresRestoreRequest {
        url: target.url().to_owned(),
        schema: schema.clone(),
        tables,
    };
    let report = restore_postgres(&request).expect("the restore must succeed");
    assert_eq!(report.schema, schema);
    let observed = observed_total(&report);
    assert_eq!(
        report.rows_loaded, observed,
        "rows_loaded is the observed total: {report:?}"
    );
    // The declared total carries the inflation, so a `rows_loaded` that still
    // held the archive's arithmetic would be at least a billion above this.
    let declared_total: u64 = INFLATED
        + facts
            .tables
            .iter()
            .filter(|table| table.table != "items")
            .map(|table| table.rows)
            .sum::<u64>();
    assert!(
        report.rows_loaded < declared_total,
        "the reported total is the observation, not the declaration: {report:?}"
    );
    assert!(
        !report.counts_ok,
        "the inflated declaration is a mismatch: {report:?}"
    );
    let items = report
        .table_counts
        .iter()
        .find(|(name, _)| name == "items")
        .expect("items in the report")
        .1;
    assert_eq!(items, 2, "items was observed, not declared: {report:?}");
}
