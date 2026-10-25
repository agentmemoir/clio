// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the concurrency coverage of the restore's
//! count: a writer that queues on the restore transaction's own table lock, and
//! the counts the report carries while that writer still waits.
//!
//! ## Owns
//! - The second-connection writer that blocks on the restore's lock
//! - The proof that the reported counts are the rows the restore loaded
//!
//! ## Does not own
//! - Restore scenario coverage without a second connection (see
//!   `postgres_restore_tests`)
//! - The count queries themselves (see `postgres_restore_state`)
//!
//! ## Boundary
//!
//! Tests here break when the restore stops counting inside its own
//! transaction, and they never change that behavior.
//!
//! Keep this module focused on the restore count under a waiting writer.

use super::tests::{base_url, empty_target, restore_tables, seeded_source};
use crate::pg_lock::lock_pg_test;
use crate::pg_tls;
use crate::postgres_backup_model::quote_ident;
use crate::postgres_restore::{PostgresRestoreReport, PostgresRestoreRequest, restore_postgres};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the lock wait and the writer's own insert may take before the test
/// fails instead of hanging. Generous: it bounds a hang, it never paces work.
const WAIT: Duration = Duration::from_secs(60);

/// The stall between two reads of the lock view. The read is a poll against
/// `WAIT`, not a synchronization delay.
const POLL: Duration = Duration::from_millis(5);

/// The probe row the waiting writer inserts; it must sort after every
/// restored bank id, so a reader of the banks sees it as the extra row.
const PROBE_BANK: &str = "zzzz-race-probe";

/// Wait until `schema`'s `banks` table is held under an exclusive lock.
///
/// The restore's `TRUNCATE` is the only statement in this test that takes that
/// lock, so its presence means the restore transaction is open and the writer
/// that runs next must queue behind it.
fn wait_for_restore_lock(url: &str, schema: &str) {
    let deadline = Instant::now() + WAIT;
    let mut client = pg_tls::connect(url).expect("lock probe connect");
    loop {
        let held: i64 = client
            .query_one(LOCK_HELD_SQL, &[&schema])
            .expect("read pg_locks")
            .get(0);
        if held > 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the restore never took its table lock"
        );
        std::thread::sleep(POLL);
    }
}

/// One `banks` row insert that queues behind the restore's lock.
fn insert_probe_bank(url: &str, schema: &str) {
    let sql = format!(
        "INSERT INTO {}.banks (id, created_at) VALUES ('{PROBE_BANK}', 't0')",
        quote_ident(schema)
    );
    pg_tls::connect(url)
        .expect("writer connect")
        .batch_execute(&sql)
        .expect("insert the probe bank");
}

/// The restored row count of one table, read on a fresh connection.
fn count_rows(url: &str, schema: &str, table: &str) -> u64 {
    let sql = format!(
        "SELECT COUNT(*) FROM {}.{}",
        quote_ident(schema),
        quote_ident(table)
    );
    let count: i64 = pg_tls::connect(url)
        .expect("count connect")
        .query_one(sql.as_str(), &[])
        .expect("count the restored rows")
        .get(0);
    u64::try_from(count).unwrap_or(0)
}

/// Start the restore on its own thread; the returned handle is always joined by
/// the test.
fn spawn_restore(request: PostgresRestoreRequest) -> JoinHandle<PostgresRestoreReport> {
    std::thread::spawn(move || restore_postgres(&request).expect("restore"))
}

/// A writer that queues on the restore's table lock cannot change the counts
/// the restore reports.
///
/// The restore runs on a second thread. Once its `TRUNCATE` holds `banks`
/// exclusively, this thread inserts a row: that insert waits for the restore
/// transaction to end, so it can only land after the restore's last read. The
/// report must therefore hold exactly the archived counts, and a fresh count
/// afterwards must hold one more row — the writer's, which the report excludes.
#[test]
fn restore_reports_its_own_rows_while_a_writer_waits_on_its_lock() {
    let _guard = lock_pg_test();
    let base = base_url();
    let (_source, facts, paths, _dir) = seeded_source("race");
    let (target, schema) = empty_target(&base);
    let archived: u64 = facts.tables.iter().map(|table| table.rows).sum();
    let archived_banks: u64 = facts
        .tables
        .iter()
        .find(|table| table.table == "banks")
        .map_or(0, |table| table.rows);
    let request = PostgresRestoreRequest {
        url: target.url().to_owned(),
        schema: schema.clone(),
        tables: restore_tables(&facts, &paths),
    };
    let restore = spawn_restore(request);
    wait_for_restore_lock(target.url(), &schema);
    insert_probe_bank(target.url(), &schema);
    let report = restore.join().expect("the restore thread");
    assert_eq!(report.rows_loaded, archived, "the report holds the load");
    assert!(report.counts_match, "counts must match: {report:?}");
    assert_eq!(
        count_rows(target.url(), &schema, "banks"),
        archived_banks + 1,
        "the waiting writer's row landed outside the report"
    );
}

/// The lock view: `n` when one session holds `schema`'s `banks` exclusively.
const LOCK_HELD_SQL: &str = "
SELECT count(*)
FROM pg_catalog.pg_locks l
JOIN pg_catalog.pg_class c ON c.oid = l.relation
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = $1 AND c.relname = 'banks'
  AND l.mode = 'AccessExclusiveLock' AND l.granted";
