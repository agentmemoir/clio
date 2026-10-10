// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres-backed tests for the
//! one-transaction dump: counts and facts of a seeded store, determinism, the
//! point-in-time snapshot, the read-only refusal, the stage-sink failure
//! contract, the facts' content boundary, and the sequence refusal.
//!
//! ## Owns
//! - The dump acceptance scenarios, run against a real Postgres store
//! - The shared helpers of these scenarios only
//!
//! ## Does not own
//! - The discovery scenarios (`postgres_discovery_tests`)
//! - The dump implementation itself (`postgres_dump`)
//!
//! ## Boundary
//!
//! Changes to how a dump is taken should update the scenarios here. Changes to
//! how the tests isolate their Postgres state should not.
//!
//! Keep this module focused on the dump tests.

use super::*;
use crate::embedding_space::embedding_space_pg_space_tests::{base_url, item};
use crate::pg_lock::lock_pg_test;
use crate::pg_scoped_schema::ScopedSchema;
use crate::store::Store;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A bank row the dump's snapshot can see: the content-exclusion probes need a
/// member with rows, not an empty one.
const SEED_BANK_SQL: &str =
    "INSERT INTO banks (id, created_at) VALUES ('am-seed-bank', '2026-01-01T00:00:00Z');";

/// The bank row the point-in-time probe commits on a second connection.
const PROBE_BANK_SQL: &str =
    "INSERT INTO banks (id, created_at) VALUES ('am-probe-bank', '2026-01-01T00:00:00Z');";

/// The write the read-only probe attempts on the dump's own transaction.
const READONLY_BANK_SQL: &str =
    "INSERT INTO banks (id, created_at) VALUES ('am-readonly-probe', '2026-01-01T00:00:00Z');";

/// The row count of one application table, read on a fresh connection.
fn count_rows(url: &str, table: &str) -> u64 {
    let mut client = crate::pg_tls::connect(url).expect("count connect");
    let sql = format!("SELECT count(*) FROM {}", quote_ident(table));
    let row = client.query_one(sql.as_str(), &[]).expect("count rows");
    u64::try_from(row.get::<_, i64>(0)).expect("a count is non-negative")
}

/// Whether one row matched, read on a fresh connection.
fn row_exists(url: &str, sql: &str) -> bool {
    let mut client = crate::pg_tls::connect(url).expect("row-exists connect");
    client
        .query_opt(sql, &[])
        .expect("run the probe read")
        .is_some()
}

/// The schema the connection's search path resolves to, read directly.
fn effective_schema_of(url: &str) -> String {
    let mut client = crate::pg_tls::connect(url).expect("connect");
    let mut tx = client.transaction().expect("transaction");
    effective_schema(&mut tx).expect("effective schema")
}

/// Run one SQL batch on a fresh connection.
fn batch(url: &str, sql: &str) {
    let mut client = crate::pg_tls::connect(url).expect("batch connect");
    client.batch_execute(sql).expect("run the batch");
}

/// A fresh staging directory, unique per call within and across processes.
fn stage_dir(tag: &str) -> PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "clio-pg-{tag}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("create the stage dir");
    dir
}

/// Open a private-schema store seeded with `count` items in one fresh bank.
fn seeded(count: usize, bank: &str) -> ScopedSchema {
    let scoped = ScopedSchema::new(&base_url());
    {
        let store = scoped.open();
        seed(&store, bank, count);
    }
    scoped
}

/// The stage sink the tests hand to a dump: one file per table, each handed
/// path recorded so the caller-contract assertions can account for them.
/// `refuse_on` (a one-based ordinal) makes the sink fail for one table.
fn tracking_stage(
    dir: PathBuf,
    handed: Rc<RefCell<Vec<PathBuf>>>,
    refuse_on: Option<usize>,
) -> impl FnMut(&PgTable, usize) -> Result<(PathBuf, std::fs::File), AmError> {
    move |table, ordinal| {
        if refuse_on == Some(ordinal) {
            return Err(AmError::new(ErrorCode::Internal, "the stage sink refused"));
        }
        let path = dir.join(format!("{ordinal:03}_{}.txt", table.name));
        let file = std::fs::File::create(&path).map_err(|error| {
            AmError::new(ErrorCode::Internal, format!("stage {path:?}: {error}"))
        })?;
        handed.borrow_mut().push(path.clone());
        Ok((path, file))
    }
}

/// The member-name and digest shape every dumped table must have: members in
/// `dump/tables/NNN_<table>.txt` form over the name-sorted tables, digests of
/// 64 lowercase hex characters.
fn assert_member_shape(facts: &PostgresDumpFacts) {
    for (index, table) in facts.tables.iter().enumerate() {
        assert_eq!(
            table.member,
            member_name(index + 1, &table.table).expect("member name"),
            "member numbering follows the name-sorted tables"
        );
        assert!(table.member.starts_with("dump/tables/"));
        assert_eq!(table.sha256.len(), 64, "digest length for {}", table.table);
        let lowercase = table
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
        assert!(lowercase, "digest is lowercase hex for {}", table.table);
    }
}

/// One dumped table's staged bytes, with its facts.
fn staged_member(
    facts: &PostgresDumpFacts,
    handed: &[PathBuf],
    table: &str,
) -> (TableDump, Vec<u8>) {
    let index = facts
        .tables
        .iter()
        .position(|t| t.table == table)
        .unwrap_or_else(|| panic!("the {table} member exists"));
    let bytes = std::fs::read(&handed[index]).expect("read the staged member");
    (facts.tables[index].clone(), bytes)
}

/// Seed `count` memory items into one fresh bank.
fn seed(store: &crate::PostgresStore, bank: &str, count: usize) {
    for i in 0..count {
        store
            .create_memory_item(&item(&format!("{bank}-item-{i}"), bank), "harness")
            .expect("seed an item");
    }
}

#[test]
fn a_seeded_store_dumps_one_member_per_app_table_with_true_counts() {
    let _guard = lock_pg_test();
    let scoped = seeded(3, "am-dump-bank");

    let dir = stage_dir("basic");
    let handed = Rc::new(RefCell::new(Vec::new()));
    let mut stage = tracking_stage(dir.clone(), Rc::clone(&handed), None);
    let facts = snapshot_postgres_into(scoped.url(), &mut stage).expect("dump the seeded store");

    assert_eq!(facts.schema, effective_schema_of(scoped.url()));
    assert_ne!(facts.schema, "public");
    assert_eq!(facts.schema_version.as_deref(), Some(crate::SCHEMA_VERSION));
    assert!(facts.banks.contains(&"am-dump-bank".to_owned()));

    let names: Vec<&str> = facts.tables.iter().map(|t| t.table.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "tables dump in name order");

    assert_member_shape(&facts);
    let handed = handed.borrow().clone();
    assert_eq!(handed.len(), facts.tables.len());
    for (path, table) in handed.iter().zip(&facts.tables) {
        let bytes = std::fs::read(path).expect("read the staged member");
        assert_eq!(u64::try_from(bytes.len()).unwrap_or(u64::MAX), table.bytes);
        #[allow(clippy::naive_bytecount)]
        let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
        assert_eq!(newlines, table.rows, "staged rows for {}", table.table);
        let source = count_rows(scoped.url(), &table.table);
        assert_eq!(table.rows, source, "source rows for {}", table.table);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn two_dumps_of_an_unchanged_store_produce_identical_members_and_digests() {
    let _guard = lock_pg_test();
    let scoped = seeded(2, "am-dump-bank");

    let dump = |tag: &str| -> PostgresDumpFacts {
        let dir = stage_dir(tag);
        let handed = Rc::new(RefCell::new(Vec::new()));
        let mut stage = tracking_stage(dir.clone(), Rc::clone(&handed), None);
        let facts = snapshot_postgres_into(scoped.url(), &mut stage).expect("dump");
        std::fs::remove_dir_all(&dir).ok();
        facts
    };
    let first = dump("again-1");
    let second = dump("again-2");

    let members = |facts: &PostgresDumpFacts| {
        facts
            .tables
            .iter()
            .map(|t| (t.member.clone(), t.sha256.clone(), t.rows, t.bytes))
            .collect::<Vec<_>>()
    };
    assert_eq!(members(&first), members(&second));
    assert_eq!(first.schema, second.schema);
    assert_eq!(first.banks, second.banks);
    assert_eq!(first.extensions, second.extensions);
    assert_eq!(first.schema_version, second.schema_version);
}

#[test]
fn the_dump_snapshot_predates_rows_committed_during_the_dump() {
    let _guard = lock_pg_test();
    let scoped = seeded(2, "am-dump-bank");
    // One bank row the snapshot can see, so the exclusion is provable by
    // content and not just by an empty member.
    batch(scoped.url(), SEED_BANK_SQL);
    let pre_banks = count_rows(scoped.url(), "banks");
    let pre_items = count_rows(scoped.url(), "items");

    let dir = stage_dir("pit");
    let handed = Rc::new(RefCell::new(Vec::new()));
    let mut stage = tracking_stage(dir.clone(), Rc::clone(&handed), None);
    let mut probe = |tx: &mut Transaction, _discovered: &DiscoveredSchema| {
        // A second connection commits new rows mid-dump; the dump's
        // REPEATABLE READ snapshot predates them.
        batch(scoped.url(), PROBE_BANK_SQL);
        let _ = tx;
        Ok(())
    };
    let facts = snapshot_postgres_with(scoped.url(), &mut stage, &mut probe).expect("dump");

    assert_eq!(count_rows(scoped.url(), "banks"), pre_banks + 1);
    let handed = handed.borrow().clone();
    let (banks, banks_bytes) = staged_member(&facts, &handed, "banks");
    assert_eq!(banks.rows, pre_banks, "banks exclude the probe row");
    assert!(
        !String::from_utf8_lossy(&banks_bytes).contains("am-probe-bank"),
        "the committed-mid-dump row must not be in the dump"
    );
    let (items, _items_bytes) = staged_member(&facts, &handed, "items");
    assert_eq!(items.rows, pre_items);
    assert!(!facts.banks.contains(&"am-probe-bank".to_owned()));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_write_on_the_dump_transaction_is_refused_and_fails_the_dump() {
    let _guard = lock_pg_test();
    let scoped = seeded(1, "am-dump-bank");

    let dir = stage_dir("readonly");
    let handed = Rc::new(RefCell::new(Vec::new()));
    let mut stage = tracking_stage(dir.clone(), Rc::clone(&handed), None);
    let refused = Rc::new(RefCell::new(String::new()));
    let refused_for_probe = Rc::clone(&refused);
    let mut probe = |tx: &mut Transaction, _discovered: &DiscoveredSchema| match tx
        .execute(READONLY_BANK_SQL, &[])
    {
        Ok(_) => Err(AmError::new(
            ErrorCode::Internal,
            "a write on the read-only snapshot was not refused",
        )),
        Err(error) => {
            let text = error
                .as_db_error()
                .map(|db| db.message().to_owned())
                .unwrap_or_else(|| error.to_string());
            *refused_for_probe.borrow_mut() = text;
            Ok(())
        }
    };
    let outcome = snapshot_postgres_with(scoped.url(), &mut stage, &mut probe);

    assert!(outcome.is_err());

    let refusal = refused.borrow();
    assert!(
        refusal.contains("read-only"),
        "the refusal names the read-only transaction: {refusal}"
    );
    assert!(
        !row_exists(
            scoped.url(),
            "SELECT 1 FROM banks WHERE id = 'am-readonly-probe'"
        ),
        "the refused write must not exist"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failing_stage_rolls_back_and_leaves_the_cleanup_to_the_caller() {
    let _guard = lock_pg_test();
    let scoped = seeded(3, "am-dump-bank");
    let pre_items = count_rows(scoped.url(), "items");

    let dir = stage_dir("stagefail");
    let handed = Rc::new(RefCell::new(Vec::new()));
    let mut stage = tracking_stage(dir.clone(), Rc::clone(&handed), Some(2));
    let outcome = snapshot_postgres_into(scoped.url(), &mut stage);

    assert!(outcome.is_err());
    // Caller contract: the dump handed out one staged path and never removed
    // it; the caller removes everything it was handed.
    let handed = handed.borrow().clone();
    assert_eq!(handed.len(), 1, "the stage sink failed on the second table");
    for path in &handed {
        std::fs::remove_file(path).expect("the staged file still exists for the caller to remove");
    }
    std::fs::remove_dir_all(&dir).ok();

    // The rollback left the source untouched and openable.
    assert_eq!(count_rows(scoped.url(), "items"), pre_items);
    let _store = scoped.open();
}

#[test]
fn row_content_stays_out_of_the_facts_and_in_the_members() {
    const SECRET: &str = "am-plant-secret-9f2c1d";
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());
    {
        let store = scoped.open();
        let mut planted = item("am-dump-secret", "am-dump-bank");
        // subject_id is a plaintext column at rest, so the member carries it
        // while every fact must not.
        planted.subject_id = format!("subject-{SECRET}");
        store
            .create_memory_item(&planted, "harness")
            .expect("seed the planted item");
    }

    let dir = stage_dir("secret");
    let handed = Rc::new(RefCell::new(Vec::new()));
    let mut stage = tracking_stage(dir.clone(), Rc::clone(&handed), None);
    let facts = snapshot_postgres_into(scoped.url(), &mut stage).expect("dump");

    let handed = handed.borrow().clone();
    let (_items, items_bytes) = staged_member(&facts, &handed, "items");
    let text = String::from_utf8_lossy(&items_bytes);
    assert!(text.contains(SECRET), "plaintext is in the member rows");
    let rendered = format!("{facts:?}");
    assert!(!rendered.contains(SECRET), "fact plaintext: {rendered}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_owned_sequence_refusal_names_the_table_and_the_sequence() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());
    batch(
        scoped.url(),
        "CREATE TABLE seq_t (id serial PRIMARY KEY, note text);",
    );

    let dir = stage_dir("seq");
    let handed = Rc::new(RefCell::new(Vec::new()));
    let mut stage = tracking_stage(dir.clone(), Rc::clone(&handed), None);
    let error = snapshot_postgres_into(scoped.url(), &mut stage).expect_err("a sequence refusal");
    assert!(error.message.contains("seq_t"), "error: {}", error.message);
    assert!(error.message.contains("seq_t_id_seq"));
    assert!(handed.borrow().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_schema_without_app_tables_refuses_to_dump() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());

    let dir = stage_dir("empty");
    let handed = Rc::new(RefCell::new(Vec::new()));
    let mut stage = tracking_stage(dir.clone(), Rc::clone(&handed), None);
    let error = snapshot_postgres_into(scoped.url(), &mut stage)
        .expect_err("a schema with nothing to copy refuses");
    assert!(error.message.contains("no application tables"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unwritable_staged_member_fails_the_copy_naming_the_table() {
    let _guard = lock_pg_test();
    let scoped = ScopedSchema::new(&base_url());
    batch(
        scoped.url(),
        "CREATE TABLE copy_t (id int);\nINSERT INTO copy_t VALUES (1);",
    );

    let dir = stage_dir("unwritable");
    // A stage sink that hands back a read-only handle: the dump's write fails,
    // which names the table and never the stream.
    let read_only = dir.join("read-only.txt");
    std::fs::write(&read_only, b"placeholder").expect("create the read-only file");
    let read_only_for_stage = read_only.clone();
    let mut stage = move |_table: &PgTable, _ordinal: usize| {
        let file = std::fs::File::open(&read_only_for_stage).expect("open the read-only handle");
        Ok((read_only_for_stage.clone(), file))
    };
    let error = snapshot_postgres_into(scoped.url(), &mut stage)
        .expect_err("an unwritable staged member fails the dump");
    assert!(error.message.contains("writing a staged table member"));
    assert!(error.message.contains("copy_t"));
    std::fs::remove_dir_all(&dir).ok();
}
