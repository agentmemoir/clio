// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Store-level parity and negative tests around the `inspect_items` metadata listing and the item reads it shares the schema with.

use crate::model::{InspectSource, InspectValidity, ItemInspectFilter, ItemInspectRow};
use crate::{DekProvider, LocalDevKms, PostgresStore, Store};
use clio_types::{AmError, EpistemicKind, ErrorCode, ItemKind, MemoryItem};
use clio_types::{SemanticCategory, SourceType};
use serde_json::json;
use std::sync::Arc;

use crate::{migrate::IntoAmError, test_unique::unique};

fn item(
    id: &str,
    bank: &str,
    cat: SemanticCategory,
    kind: EpistemicKind,
    source: Option<SourceType>,
) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(cat),
        episodic_type: None,
        epistemic_kind: kind,
        source_type: source,
        confidence: Some(0.6),
        admission_score: Some(0.5),
        source_ref: Some("ev".into()),
        snapshot: Some(json!({"k": id})),
        gist: Some("g".into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    }
}

fn ids(rows: &[ItemInspectRow]) -> Vec<String> {
    rows.iter().map(|r| r.item_id.clone()).collect()
}

#[test]
fn inspect_items_parity_and_filters() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = crate::postgres::PostgresStore::open(url.as_str(), kms.clone()).expect("pg");
    let lite = crate::sqlite::SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-inspect");
    let (fact_id, belief_id) = (unique("fact"), unique("belief"));
    for store in [&lite as &dyn Store, &pg] {
        store
            .create_memory_item(
                &item(
                    &fact_id,
                    &bank,
                    SemanticCategory::ToolConfig,
                    EpistemicKind::Fact,
                    None,
                ),
                "agent",
            )
            .expect("fact");
        store
            .create_memory_item(
                &item(
                    &belief_id,
                    &bank,
                    SemanticCategory::Schema,
                    EpistemicKind::Belief,
                    Some(SourceType::UserStated),
                ),
                "agent",
            )
            .expect("belief");
    }

    let all = ItemInspectFilter::new(&bank);
    let lite_all = lite.inspect_items(&all).expect("sqlite all");
    let pg_all = pg.inspect_items(&all).expect("pg all");
    assert_eq!(ids(&lite_all), ids(&pg_all));
    assert_eq!(lite_all.len(), 2);
    // Total count mirrors the predicates on both backends.
    assert_eq!(lite.count_items(&all).expect("sqlite count"), 2);
    assert_eq!(pg.count_items(&all).expect("pg count"), 2);
    let belief_row = pg_all
        .iter()
        .find(|r| r.item_id == belief_id)
        .expect("belief row");
    assert_eq!(belief_row.confidence, Some(0.6));
    assert_eq!(belief_row.source_type.as_deref(), Some("user_stated"));
    let fact_row = pg_all
        .iter()
        .find(|r| r.item_id == fact_id)
        .expect("fact row");
    assert_eq!(fact_row.admission_score, Some(0.5));
    assert!(pg_all.iter().all(|r| !r.discarded && r.tx_until.is_none()));

    let mut beliefs = ItemInspectFilter::new(&bank);
    beliefs.epistemic_kind = Some("belief".into());
    let pg_beliefs = pg.inspect_items(&beliefs).expect("pg belief");
    assert_eq!(ids(&pg_beliefs), vec![belief_id.clone()]);
    let lite_beliefs = ids(&lite.inspect_items(&beliefs).expect("sqlite belief"));
    assert_eq!(lite_beliefs, ids(&pg_beliefs));

    // Pagination is deterministic on both backends; total ignores the page.
    let mut page = ItemInspectFilter::new(&bank);
    page.limit = 1;
    page.offset = 1;
    assert_eq!(ids(&pg.inspect_items(&page).expect("pg page")).len(), 1);
    let lite_page = ids(&lite.inspect_items(&page).expect("sqlite page"));
    let pg_page = ids(&pg.inspect_items(&page).expect("pg page 2"));
    assert_eq!(lite_page, pg_page);
    assert_eq!(pg.count_items(&page).expect("pg page count"), 2);

    // Half-open `[start, end)`: an end at `created_at` excludes the row on both backends; a later end includes it.
    let mut half_open = ItemInspectFilter::new(&bank);
    half_open.time_start = Some("2024-01-01T00:00:00Z".into());
    half_open.time_end = Some("2024-01-01T00:00:00Z".into());
    let lite_half_open_rows = lite.inspect_items(&half_open).expect("sqlite half-open");
    assert!(lite_half_open_rows.is_empty(), "{lite_half_open_rows:?}");
    let pg_half_open_rows = pg.inspect_items(&half_open).expect("pg half-open");
    assert!(pg_half_open_rows.is_empty(), "{pg_half_open_rows:?}");
    let mut window = ItemInspectFilter::new(&bank);
    window.time_start = Some("2024-01-01T00:00:00Z".into());
    window.time_end = Some("2024-01-02T00:00:00Z".into());
    assert_eq!(pg.inspect_items(&window).expect("pg window").len(), 2);

    // Invalidated filter excludes the still-active rows.
    let mut invalidated = ItemInspectFilter::new(&bank);
    invalidated.validity = InspectValidity::Invalidated;
    let invalidated_rows = pg.inspect_items(&invalidated).expect("pg invalidated");
    assert!(invalidated_rows.is_empty(), "{invalidated_rows:?}");

    // Clean up rows and index-queue entries so the shared whole-store index contract is not perturbed.
    for store in [&lite as &dyn Store, &pg] {
        store.delete_item(&bank, &fact_id).expect("delete fact");
        store.delete_item(&bank, &belief_id).expect("delete belief");
    }
}

/// Scalar probe against the store's own SQLite connection (index presence).
fn sqlite_index_present(store: &crate::sqlite::SqliteStore, name: &str) -> bool {
    let sql =
        format!("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = '{name}'");
    store
        .with_conn(|conn| {
            let count: i64 = conn.query_row(&sql, [], |r| r.get(0)).am("index probe")?;
            Ok(count > 0)
        })
        .expect("sqlite index probe")
}

const PG_PROBE: &str = "SELECT COUNT(*)::bigint FROM pg_indexes WHERE schemaname = current_schema() AND indexname = $1";

/// Index presence in the store's current Postgres schema (name is unique per schema, so the probe is schema-qualified).
fn pg_index_present(store: &crate::postgres::PostgresStore, name: &str) -> bool {
    store
        .with_client(|client| {
            let row = client.query_one(PG_PROBE, &[&name]).am("pg index probe")?;
            Ok(row.get::<_, i64>(0) > 0)
        })
        .expect("pg index probe")
}

#[test]
fn source_type_filter_counts_and_index_parity() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = crate::postgres::PostgresStore::open(url.as_str(), kms.clone()).expect("pg");
    let path = std::env::temp_dir().join(format!("am-inspect-{}.db", unique("parity")));
    let lite = crate::sqlite::SqliteStore::open(&path, kms.clone()).expect("sqlite");
    let bank = unique("bank-source");
    // Fixture: 1 fact + 1 belief per provenance rung; hand-computed counts:
    // any = 4, each rung = 1.
    let fixture = [
        (unique("fact"), EpistemicKind::Fact, None),
        (
            unique("user"),
            EpistemicKind::Belief,
            Some(SourceType::UserStated),
        ),
        (
            unique("agent"),
            EpistemicKind::Belief,
            Some(SourceType::AgentInferred),
        ),
        (
            unique("third"),
            EpistemicKind::Belief,
            Some(SourceType::ThirdParty),
        ),
    ];
    for store in [&lite as &dyn Store, &pg] {
        for (id, kind, source) in &fixture {
            store
                .create_memory_item(
                    &item(id, &bank, SemanticCategory::Persona, *kind, *source),
                    "agent",
                )
                .expect("fixture item");
        }
    }

    // Re-run the schema path on each existing database: reopening re-executes the bootstrap against
    // what it already built, so the index probe below pins idempotent creation, not first-run success.
    drop((lite, pg));
    let pg = crate::postgres::PostgresStore::open(url.as_str(), kms.clone()).expect("pg reopen");
    let lite = crate::sqlite::SqliteStore::open(&path, kms.clone()).expect("sqlite reopen");

    // Absent means any.
    let any = ItemInspectFilter::new(&bank);
    assert_eq!(lite.count_items(&any).expect("sqlite any"), 4, "sqlite any");
    assert_eq!(pg.count_items(&any).expect("pg any"), 4, "pg any");

    // Each rung selects exactly its own row, on both backends.
    let rungs = [
        (InspectSource::Fact, 0),
        (InspectSource::UserStated, 1),
        (InspectSource::AgentInferred, 2),
        (InspectSource::ThirdParty, 3),
    ];
    for (rung, slot) in rungs {
        let want_id = &fixture[slot].0;
        let mut filter = ItemInspectFilter::new(&bank);
        filter.source_type = Some(rung);
        let got = lite.count_items(&filter).expect("sqlite count");
        assert_eq!(got, 1, "sqlite {rung:?}");
        assert_eq!(pg.count_items(&filter).expect("pg count"), 1, "pg {rung:?}");
        let pg_rows = pg.inspect_items(&filter).expect("pg rows");
        assert_eq!(ids(&pg_rows), vec![want_id.clone()], "pg {rung:?}");
        let lite_rows = ids(&lite.inspect_items(&filter).expect("sqlite rows"));
        assert_eq!(lite_rows, vec![want_id.clone()], "sqlite {rung:?}");
    }

    // The provenance index exists on both backends after the schema path ran twice on the same database.
    assert!(
        pg_index_present(&pg, "items_bank_id_source_type_inx"),
        "pg provenance index missing"
    );
    assert!(
        sqlite_index_present(&lite, "items_bank_id_source_type_inx"),
        "sqlite provenance index missing"
    );

    // Clean up: remove the rows and their index-queue entries so this test does not perturb the shared
    // whole-store index contract.
    for store in [&lite as &dyn Store, &pg] {
        for (id, _, _) in &fixture {
            store.delete_item(&bank, id).expect("delete fixture item");
        }
    }
    drop((lite, pg));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db.lock"));
}

/// The read of a stored out-of-constraint `source_type` must fail with the parse error, never
/// classify, and no provenance rung may count the row.
fn assert_out_of_constraint_is_an_error(store: &dyn Store, bank: &str, item_id: &str) {
    let error = store
        .get_memory_item(bank, item_id)
        .expect_err("an out-of-constraint stored source_type must fail the read, not classify");
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error}");
    assert!(error.message.contains("source_type must be"), "{error}");
    for rung in [
        InspectSource::Fact,
        InspectSource::UserStated,
        InspectSource::AgentInferred,
        InspectSource::ThirdParty,
    ] {
        let mut filter = ItemInspectFilter::new(bank);
        filter.source_type = Some(rung);
        assert_eq!(
            store.count_items(&filter).expect("rung count"),
            0,
            "an out-of-constraint value counts as no class: {rung:?}"
        );
    }
}

/// Restores the damaged row and the dropped CHECK even when the test body panics between the drop and the restore; the shared database keeps neither change.
struct OocPgRestore<'a> {
    pg: &'a PostgresStore,
    item: String,
    source: Option<String>,
    drop_sql: String,
    add_sql: String,
    done: bool,
}

impl OocPgRestore<'_> {
    fn restore(&self) -> Result<(), AmError> {
        self.pg.with_client(|client| {
            client
                .execute(
                    "UPDATE items SET source_type = $2 WHERE id = $1",
                    &[&self.item, &self.source],
                )
                .am("restore the damaged row")?;
            client.execute(&self.add_sql, &[]).am("restore the CHECK")?;
            Ok(())
        })
    }
}

impl Drop for OocPgRestore<'_> {
    fn drop(&mut self) {
        if !self.done
            && let Err(e) = self.restore()
        {
            eprintln!("post-panic pg restore failed: {e}");
        }
    }
}

const PG_CHECK_LIST: &str = "SELECT conname, pg_get_constraintdef(oid) FROM pg_constraint WHERE conrelid = 'items'::regclass AND contype = 'c'";

/// Captures the `source_type` CHECK definition and the row's stored value, drops the CHECK, and stores the forbidden
/// value, returning the guard that restores both; the definition mentioning `source_type` without `epistemic_kind` separates it from the fact/belief CHECK beside it.
fn damage_pg_source_type_check<'a>(pg: &'a PostgresStore, belief_id: &str) -> OocPgRestore<'a> {
    let (drop_sql, add_sql, row_source) = pg
        .with_client(|client| {
            let rows = client
                .query(PG_CHECK_LIST, &[])
                .am("list items CHECK constraints")?;
            let mut named: Vec<(String, String)> = rows
                .iter()
                .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
                .collect();
            named.retain(|(_, def)| def.contains("source_type") && !def.contains("epistemic_kind"));
            if named.len() != 1 {
                return Err(AmError::new(
                    ErrorCode::Internal,
                    format!("expected exactly one source_type CHECK on items, got {named:?}"),
                ));
            }
            let (name, def) = named.remove(0);
            let row = client
                .query_one("SELECT source_type FROM items WHERE id = $1", &[&belief_id])
                .am("read the row's stored source_type")?;
            let add_sql = format!("ALTER TABLE items ADD CONSTRAINT {name} {def}");
            let drop_sql = format!("ALTER TABLE items DROP CONSTRAINT {name}");
            Ok((drop_sql, add_sql, row.get::<_, Option<String>>(0)))
        })
        .expect("find the source_type CHECK and the row value");
    let restore = OocPgRestore {
        pg,
        item: belief_id.into(),
        source: row_source,
        drop_sql,
        add_sql,
        done: false,
    };
    pg.with_client(|client| {
        client
            .execute(&restore.drop_sql, &[])
            .am("drop the CHECK")?;
        client
            .execute(
                "UPDATE items SET source_type = 'hallucinated' WHERE id = $1",
                &[&belief_id],
            )
            .am("store the forbidden value")?;
        Ok(())
    })
    .expect("damage the pg row");
    restore
}

/// Relaxes the SQLite `source_type` CHECK under `writable_schema` on its own connection, then stores the
/// forbidden value from a fresh raw connection; the store's own connection never writes the value.
fn damage_sqlite_source_type(path: &std::path::Path, belief_id: &str) {
    let relabeled = rusqlite::Connection::open(path).expect("open raw sqlite");
    relabeled
        .execute_batch("PRAGMA writable_schema = ON;")
        .expect("make the schema writable");
    let changed = relabeled
        .execute(
            "UPDATE sqlite_master SET sql = replace(sql, ?1, '') WHERE type = 'table' AND name = 'items'",
            rusqlite::params![
                "CHECK (source_type IS NULL OR source_type IN ('user_stated', 'agent_inferred', 'third_party'))"
            ],
        )
        .expect("relax the source_type CHECK");
    assert_eq!(changed, 1, "the items table must carry the relaxed CHECK");
    relabeled
        .execute_batch("PRAGMA writable_schema = OFF;")
        .expect("close the schema write");
    drop(relabeled);
    let damaged = rusqlite::Connection::open(path).expect("reopen raw sqlite");
    let updated = damaged
        .execute(
            "UPDATE items SET source_type = 'hallucinated' WHERE id = ?1",
            rusqlite::params![belief_id],
        )
        .expect("store the forbidden value");
    assert_eq!(updated, 1, "the damaged row must be updated");
    drop(damaged);
}

/// A stored `source_type` the schema CHECK forbids fails the item read with the parse error, and no
/// provenance rung counts the row: an out-of-constraint value is a storage error, never a class.
/// The write API cannot produce such a row, so the test relaxes the CHECK the way storage drift
/// would, restores each backend, and throws the damaged file away.
#[test]
fn an_out_of_constraint_source_type_is_an_error_not_a_class() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = crate::postgres::PostgresStore::open(url.as_str(), kms.clone()).expect("pg");
    let path = std::env::temp_dir().join(format!("am-inspect-{}.db", unique("ooc")));
    let lite = crate::sqlite::SqliteStore::open(&path, kms.clone()).expect("sqlite");
    let bank = unique("bank-ooc");
    let belief_id = unique("belief-ooc");
    let belief = item(
        &belief_id,
        &bank,
        SemanticCategory::Schema,
        EpistemicKind::Belief,
        Some(SourceType::UserStated),
    );
    for store in [&lite as &dyn Store, &pg] {
        store.create_memory_item(&belief, "agent").expect("belief");
    }

    let mut restore = damage_pg_source_type_check(&pg, &belief_id);
    assert_out_of_constraint_is_an_error(&pg, &bank, &belief_id);

    // Restore the row and the constraint before anything else touches the shared database; the guard repeats this after a panic.
    restore.restore().expect("restore the pg constraint");
    restore.done = true;

    damage_sqlite_source_type(&path, &belief_id);
    assert_out_of_constraint_is_an_error(&lite, &bank, &belief_id);

    drop(lite);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db.lock"));
}
