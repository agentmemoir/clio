// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres-backed scenario tests of the
//! restore transaction: the round trip into an empty target, the rollback on
//! members that fail to load, every pre-flight refusal, the trigger-disabled
//! load, and the count report measured inside the transaction — against the shared Compose DB.
//!
//! ## Owns
//! - Restore scenario coverage for `restore_postgres` and `postgres_target_state`
//! - The staged-dump harness (a scratch directory of staged members) the
//!   scenarios need to build a restore request from a real dump
//!
//! ## Does not own
//! - Dump-path coverage (`postgres_dump_tests`)
//! - The pure table-set comparison (`postgres_backup_model_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the restore's transaction order, refusals, or
//! reporting change; they never change that behavior.
//!
//! Keep this module focused on restore scenario coverage.

use crate::belief_store::BeliefStore;
use crate::db_url::postgres_url_from_env;
use crate::migrate::map_db_err;
use crate::pg_lock::lock_pg_test;
use crate::pg_scoped_schema::ScopedSchema;
use crate::postgres::PostgresStore;
use crate::postgres_backup_model::{PgTable, quote_ident};
use crate::postgres_dump::{PostgresDumpFacts, snapshot_postgres_into};
use crate::postgres_restore::restore_postgres;
use crate::postgres_restore::{PostgresRestoreReport, PostgresRestoreRequest, RestoreTable};
use crate::postgres_restore_state::postgres_target_state;
use crate::store::Store;
use clio_types::{
    AmError, BeliefObject, ConfidenceEntry, EpistemicKind, ErrorCode, ItemKind, MemoryItem,
    SemanticCategory, SourceType,
};
use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn base_url() -> String {
    postgres_url_from_env()
}

fn unique(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{tag}_{}_{}", std::process::id(), nanos)
}

/// One item with content strings a round trip can read back.
fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{bank}"),
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

/// One belief; `create_belief` also writes the confidence entry that makes
/// the `belief_confidence_entries` member non-empty.
fn belief(id: &str, bank: &str) -> BeliefObject {
    let entry = ConfidenceEntry {
        as_of: "2026-01-01T00:00:00Z".into(),
        confidence: 0.4,
        evidence_ref: "turn_1".into(),
        source_type: SourceType::UserStated,
    };
    BeliefObject {
        context: None,
        context_as_of: None,
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj-restore".into(),
        proposition: "restore probe belief".into(),
        confidence_history: vec![entry],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

/// Seed three items across two banks and one belief. Returns the `(id,
/// bank_id)` pairs in `id` order, so a restored read-back can compare them
/// against `ORDER BY id`.
fn seed(store: &PostgresStore, tag: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for (letter, number) in [("a", "1"), ("b", "1"), ("c", "2")] {
        let id = format!("{letter}-{tag}");
        let bank = format!("{tag}-{number}");
        store
            .create_memory_item(&item(&id, &bank), "harness")
            .expect("seed item");
        rows.push((id, bank));
    }
    let bank = format!("{tag}-1");
    let probe = belief(&format!("bel-{tag}"), &bank);
    store.create_belief(&probe, "harness").expect("seed belief");
    rows
}

/// A scratch directory for staged members, removed on drop.
pub(super) struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(unique(&format!("clio-restore-{tag}")));
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Self { path }
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Dump `url`'s store into one staged file per table in a scratch directory.
/// Returns the facts and the staged paths, in the dump's member order.
fn dump_into_scratch(url: &str, tag: &str) -> (PostgresDumpFacts, Vec<PathBuf>, ScratchDir) {
    let dir = ScratchDir::new(tag);
    let mut paths: Vec<PathBuf> = Vec::new();
    let facts = snapshot_postgres_into(url, &mut |table: &PgTable, ordinal: usize| {
        let path = dir.path.join(format!("{ordinal:03}_{}.txt", table.name));
        let file = std::fs::File::create(&path)
            .map_err(|error| map_db_err("stage a dump member", error))?;
        paths.push(path.clone());
        Ok((path, file))
    })
    .expect("dump the store");
    (facts, paths, dir)
}

/// A seeded source schema with its store dumped into staged members.
pub(super) fn seeded_source(
    tag: &str,
) -> (ScopedSchema, PostgresDumpFacts, Vec<PathBuf>, ScratchDir) {
    let base = base_url();
    let source = ScopedSchema::new(&base);
    let store = source.open();
    seed(&store, tag);
    drop(store);
    let (facts, paths, dir) = dump_into_scratch(source.url(), tag);
    (source, facts, paths, dir)
}

/// The restore input the archive's facts describe.
pub(super) fn restore_tables(facts: &PostgresDumpFacts, paths: &[PathBuf]) -> Vec<RestoreTable> {
    facts
        .tables
        .iter()
        .zip(paths)
        .map(|(table, path)| RestoreTable {
            table: table.table.clone(),
            columns: table.columns.iter().map(|c| c.name.clone()).collect(),
            member_path: path.clone(),
            rows: table.rows,
        })
        .collect()
}

fn restore_request(url: &str, schema: &str, tables: Vec<RestoreTable>) -> PostgresRestoreRequest {
    PostgresRestoreRequest {
        url: url.to_owned(),
        schema: schema.to_owned(),
        tables,
    }
}

/// Run the restore and panic with the message on failure.
fn run_restore(url: &str, schema: &str, tables: Vec<RestoreTable>) -> PostgresRestoreReport {
    restore_postgres(&restore_request(url, schema, tables)).expect("restore")
}

/// Run the restore expecting failure; panic if it succeeds.
fn restore_err(url: &str, schema: &str, tables: Vec<RestoreTable>) -> AmError {
    restore_postgres(&restore_request(url, schema, tables)).expect_err("restore must fail")
}

/// The member ordinal of one table in the dump facts.
fn member_ordinal(facts: &PostgresDumpFacts, name: &str) -> usize {
    let position = facts.tables.iter().position(|t| t.table == name);
    position.expect("member")
}

/// The schema the scoped URL's search path resolves to.
fn schema_of(url: &str) -> String {
    let mut client = crate::pg_tls::connect(url).expect("schema probe connect");
    let row = client
        .query_one("SELECT current_schema()", &[])
        .expect("read current_schema");
    row.get::<_, Option<String>>(0)
        .expect("the scoped search_path resolves to a schema")
}

/// A second scoped schema whose store tables exist and hold zero rows, with
/// the schema name the restore loads into.
pub(super) fn empty_target(base: &str) -> (ScopedSchema, String) {
    let target = ScopedSchema::new(base);
    target.open();
    let schema = schema_of(target.url());
    (target, schema)
}

/// Every observed count equals the archive's recorded count, and the report's
/// total is the sum of those observations — which, on a clean restore, is
/// also the sum of the archive's counts.
fn assert_counts_match_archive(report: &PostgresRestoreReport, facts: &PostgresDumpFacts) {
    let archived = facts.tables.iter().map(|t| t.rows).sum::<u64>();
    let observed = report
        .table_counts
        .iter()
        .map(|(_, count)| count)
        .sum::<u64>();
    assert_eq!(report.rows_loaded, observed, "the total is measured");
    assert_eq!(report.rows_loaded, archived, "and it matches the archive");
    assert!(report.counts_ok, "counts must match: {report:?}");
    for table in &facts.tables {
        let observed = report
            .table_counts
            .iter()
            .find(|(name, _)| name == &table.table)
            .map(|(_, count)| *count)
            .unwrap_or_else(|| panic!("table {} missing from the report", table.table));
        assert_eq!(observed, table.rows, "table {}", table.table);
    }
    assert_eq!(report.banks, facts.banks, "restored banks");
}

/// Insert a triple whose `item_id` has no items row. The foreign key lives in
/// triggers on `triples`, so this must fail whenever enforcement is on — the
/// behavioral proof the restore re-enabled the triggers it disabled.
fn orphan_triple_insert_must_fail(url: &str, schema: &str) {
    let sql = format!(
        "INSERT INTO {}.triples (id, bank_id, item_id, subject, predicate, object_ciphertext, \
         epistemic_kind, valid_from, tx_from, created_at) \
         VALUES ('orphan-probe', 'b', 'no-such-item', 's', 'p', 'sealed', 'fact', 't0', 't0', 't0')",
        quote_ident(schema)
    );
    let attempted = crate::pg_tls::connect(url)
        .expect("orphan probe connect")
        .batch_execute(&sql);
    assert!(attempted.is_err(), "the orphan triple must fail");
}

/// `url` with the credentials swapped for `role`'s.
fn with_credentials(url: &str, role: &str, password: &str) -> String {
    let scheme_end = url.find("://").expect("a URL with a scheme") + 3;
    let at = scheme_end + url[scheme_end..].find('@').expect("credentials in the URL");
    format!("{}{role}:{password}{}", &url[..scheme_end], &url[at..])
}

/// The full round trip into an empty second schema. The
/// archive's counts come back exactly (including the banks), the first items
/// read back byte-identical, and the load succeeds even though the archive's
/// name order puts `belief_confidence_entries` — whose rows reference
/// `beliefs` — before `beliefs`, which is possible only with the triggers
/// disabled. Afterwards an orphan triple insert fails: enforcement is back on.
#[test]
fn restore_round_trip_into_an_empty_target() {
    let _guard = lock_pg_test();
    let base = base_url();
    let source = ScopedSchema::new(&base);
    let store = source.open();
    let items = seed(&store, "ra");
    drop(store);
    let (facts, paths, _dir) = dump_into_scratch(source.url(), "ra");
    let entry = member_ordinal(&facts, "belief_confidence_entries");
    let beliefs = member_ordinal(&facts, "beliefs");
    assert!(entry < beliefs, "entries load first");
    assert!(facts.tables[entry].rows > 0);
    let target = ScopedSchema::new(&base);
    target.open();
    let empty = postgres_target_state(target.url()).expect("empty target state");
    assert!(empty.is_empty() && empty.table_count() > 0);
    let schema = schema_of(target.url());
    let report = run_restore(target.url(), &schema, restore_tables(&facts, &paths));
    assert_counts_match_archive(&report, &facts);
    orphan_triple_insert_must_fail(target.url(), &schema);
    let s = quote_ident(&schema);
    let sql = format!("SELECT id, bank_id FROM {s}.items ORDER BY id LIMIT 2");
    let rows = crate::pg_tls::connect(target.url())
        .expect("read-back connect")
        .query(&sql, &[])
        .expect("read back the restored items");
    let restored: Vec<(String, String)> = rows.iter().map(|row| (row.get(0), row.get(1))).collect();
    assert_eq!(restored, items[..2], "items read back identically");
}

/// a member that fails to load rolls the restore back — the populated
/// target keeps its rows and its foreign keys stay enforced. Three failures:
/// a member with a corrupt row, one whose open succeeds but whose first read
/// fails (a directory), and one that was never staged.
#[test]
fn member_read_failures_roll_the_restore_back() {
    let _guard = lock_pg_test();
    let (source, facts, paths, dir) = seeded_source("rb");
    let url = source.url();
    let before = postgres_target_state(url).expect("target state before");
    let mut tables = restore_tables(&facts, &paths);
    let index = member_ordinal(&facts, "items");
    assert!(facts.tables[index].rows > 0, "items member has rows");
    std::fs::write(&paths[index], b"corrupt\trow\n").expect("corrupt the member");
    let error = restore_err(url, &facts.schema, tables.clone());
    assert!(error.message.contains("items"), "names the table: {error}");
    tables[index].member_path = dir.path.clone();
    let error = restore_err(url, &facts.schema, tables.clone());
    assert!(error.message.contains("items"), "names the table: {error}");
    tables[index].member_path = dir.path.join("no-such-member.txt");
    let error = restore_err(url, &facts.schema, tables);
    assert!(error.message.contains("items"), "names the table: {error}");
    let after = postgres_target_state(url).expect("target state after");
    assert_eq!(before, after, "target untouched");
    orphan_triple_insert_must_fail(url, &facts.schema);
}

/// The restore refuses before truncating when the archive's table
/// set does not match the target — a table the target does not have, a schema
/// whose store was never opened (nothing discovered), and a member whose
/// column list the pre-flight accepts but `COPY` rejects — and the target
/// keeps its rows.
#[test]
fn table_set_mismatches_refuse_before_truncating() {
    let _guard = lock_pg_test();
    let (_source, facts, paths, _dir) = seeded_source("rc");
    let base = base_url();
    let target = ScopedSchema::new(&base);
    let target_store = target.open();
    seed(&target_store, "rc-target");
    drop(target_store);
    let schema = schema_of(target.url());
    let before = postgres_target_state(target.url()).expect("target state before");
    assert!(before.count_of("items").unwrap_or(0) > 0);
    let mut tables = restore_tables(&facts, &paths);
    tables.last_mut().expect("archive tables").table = "not_in_target".into();
    let error = restore_err(target.url(), &schema, tables.clone());
    assert!(error.message.contains("not_in_target"));
    let other = ScopedSchema::new(&base);
    let other_schema = schema_of(other.url());
    let error = restore_err(other.url(), &other_schema, tables);
    assert!(error.message.contains("does not match the target"));
    // A duplicated column passes the pre-flight (every column exists) and
    // fails when the load starts.
    let mut tables = restore_tables(&facts, &paths);
    let first = tables[0].table.clone();
    let duplicate = tables[0].columns[0].clone();
    tables[0].columns.push(duplicate);
    let error = restore_err(target.url(), &schema, tables);
    assert!(error.message.contains(&first), "names the table: {error}");
    let after = postgres_target_state(target.url()).expect("target state after");
    assert_eq!(before, after, "target untouched");
}

/// Rows the target gains outside the archive show up as
/// `counts_ok == false`. The only statement the restore runs with triggers
/// still enabled is the `TRUNCATE`, so the poison is an `AFTER TRUNCATE`
/// trigger on `items` that adds a `banks` row; an `AFTER INSERT` poison cannot
/// fire because the load runs with every trigger disabled.
#[test]
fn post_commit_count_mismatch_is_reported() {
    let _guard = lock_pg_test();
    let (_source, facts, paths, _dir) = seeded_source("re");
    let (target, schema) = empty_target(&base_url());
    let s = quote_ident(&schema);
    crate::pg_tls::connect(target.url())
        .expect("poison connect")
        .batch_execute(&format!(
            "CREATE OR REPLACE FUNCTION {s}.poison_banks() RETURNS trigger LANGUAGE plpgsql \
             AS $f$ BEGIN INSERT INTO {s}.banks (id, created_at) VALUES ('poison-bank', 't0'); \
             RETURN NULL; END $f$;
             CREATE TRIGGER poison_banks AFTER TRUNCATE ON {s}.items FOR EACH STATEMENT \
             EXECUTE FUNCTION {s}.poison_banks();"
        ))
        .expect("create the poison trigger");
    let report = run_restore(target.url(), &schema, restore_tables(&facts, &paths));
    assert!(!report.counts_ok, "poisoned counts: {report:?}");
    let archived = facts
        .tables
        .iter()
        .find(|t| t.table == "banks")
        .expect("a banks member")
        .rows;
    let observed = report
        .table_counts
        .iter()
        .find(|(name, _)| name == "banks")
        .expect("banks in the report")
        .1;
    assert_eq!(observed, archived + 1, "the poison row survived the load");
}

/// a LOGIN role with only USAGE and SELECT — but not ownership — is
/// refused with Forbidden, naming the tables, before anything is truncated.
#[test]
fn non_owning_role_is_refused_before_any_truncation() {
    let _guard = lock_pg_test();
    let (source, facts, paths, _dir) = seeded_source("rf");
    let schema = schema_of(source.url());
    let url = source.url();
    let before = postgres_target_state(url).expect("target state before");
    let role = unique("restore_probe_role");
    let probe = format!("probe-{:016x}", rand::random::<u64>());
    let s = quote_ident(&schema);
    let r = quote_ident(&role);
    let mut admin = crate::pg_tls::connect(&base_url()).expect("admin connect");
    admin
        .batch_execute(&format!(
            "CREATE ROLE {r} LOGIN PASSWORD '{probe}';
             GRANT USAGE ON SCHEMA {s} TO {r};
             GRANT SELECT ON ALL TABLES IN SCHEMA {s} TO {r};"
        ))
        .expect("create the probe role");
    let probe_url = with_credentials(url, &role, &probe);
    let tables = restore_tables(&facts, &paths);
    let error = restore_err(&probe_url, &schema, tables);
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert!(error.message.contains("items"), "names the tables: {error}");
    let after = postgres_target_state(url).expect("target state after");
    assert_eq!(before, after, "target untouched");
    admin
        .batch_execute(&format!("DROP OWNED BY {r}; DROP ROLE {r};"))
        .expect("drop the probe role");
}
