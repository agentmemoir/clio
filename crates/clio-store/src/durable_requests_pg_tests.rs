// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the durable request store on Postgres: save and
//! read-back after a reopen, refusal of a reused id bound to a different
//! scope or kind, and one-winner races on a single id.
//!
//! ## Owns
//! - The scratch-Postgres database fixture and its drop guard
//! - The Postgres save/reopen/refusal/race assertions
//!
//! ## Does not own
//! - The store implementation (`durable_requests`)
//! - The slot DDL text (`migrate` and `sql/003_local_compliance_slots.sql`)
//! - Shared-Postgres test serialization (`pg_lock`)
//! - The SQLite backend assertions (`durable_requests_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the save/reuse rules, the read-back shape, or the
//! restart behavior changes. The request vocabulary and its decoding belong
//! to the consumer's own suites.
//!
//! Test-only. Never compiled into production builds.

use crate::durable_requests::{DurableRequestRow, DurableRequestStore};
use crate::erase_test_support::local_kms;
use crate::migrate::IntoAmError;
use crate::pg_scoped_schema::ScopedSchema;
use crate::postgres::PostgresStore;
use clio_types::{AmError, ErrorCode};

/// The canonical scope serialization of a bank-wide request.
const BANK_SCOPE: &str = r#"{"bank":{"bank_id":"b1"}}"#;

/// The canonical scope serialization of a single-subject request.
const SUBJECT_SCOPE: &str = r#"{"subject":{"bank_id":"b1","subject_id":"s1"}}"#;

/// A row with fixed field values; override one field per test as needed.
fn row(request_id: &str, scope: &str) -> DurableRequestRow {
    DurableRequestRow {
        request_id: request_id.to_owned(),
        kind: "erasure".to_owned(),
        state: "received".to_owned(),
        scope: scope.to_owned(),
        deadline: None,
        record: format!(r#"{{"request_id":"{request_id}","scope":{scope}}}"#),
    }
}

/// An in-progress row: an operator deadline and a per-path saved attempt and
/// action id inside the record payload, as a restart must find them.
fn saved_row() -> DurableRequestRow {
    let mut saved = row("req-1", SUBJECT_SCOPE);
    saved.state = "in_progress".to_owned();
    saved.deadline = Some("2026-02-01T00:00:00Z".to_owned());
    saved.record =
        r#"{"paths":{"p1":{"owner":"clio","state":{"saved":{"attempt_id":"a1","action_id":"act-1"}}}}}"#
            .to_owned();
    saved
}

/// Demand success on a write, naming the step in the panic.
fn want(result: Result<(), AmError>, what: &str) {
    result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
}

/// The shared Postgres URL, or `None` after noting why the suite skips (the
/// `postgres_triple_tests` fixture shape: the note names the suite and the
/// server's reason, never the connection string).
fn shared_url() -> Option<String> {
    let url = crate::db_url::postgres_url_from_env();
    if let Err(err) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip("postgres durable request tests", &err);
        return None;
    }
    Some(url)
}

/// A database created for one test, dropped when the test ends — including
/// when it ends in a panic (the `migrate_compliance_slots_tests` fixture
/// shape). A fresh database guarantees the real durable_requests columns.
struct ScratchDatabase {
    name: String,
}

impl ScratchDatabase {
    fn create(admin: &mut postgres::Client, label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let name = format!("clio_dreq_{label}_{}_{nanos}", std::process::id());
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {name}"))
            .expect("drop a leftover scratch database");
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .expect("create a scratch database");
        Self { name }
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        if let Ok(mut admin) = crate::pg_tls::connect(&crate::db_url::postgres_url_from_env()) {
            let _ = admin.batch_execute(&format!("DROP DATABASE IF EXISTS {}", self.name));
        }
    }
}

/// The shared URL pointed at a different database on the same server.
fn url_for_database(name: &str) -> String {
    crate::db_url::with_database(&crate::db_url::postgres_url_from_env(), name)
}

fn open_pg(url: &str) -> Option<PostgresStore> {
    match PostgresStore::open(url, local_kms()) {
        Ok(store) => Some(store),
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("postgres durable request tests", &err);
            None
        }
    }
}

/// A Postgres record saves and reads back identically after a reopen.
#[test]
fn postgres_a_record_saves_and_reads_back_after_a_reopen() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "roundtrip");
    drop(admin);
    {
        let Some(store) = open_pg(&url_for_database(&db.name)) else {
            return;
        };
        want(store.save(&saved_row()), "save");
    }
    let Some(reopened) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    assert_eq!(
        reopened.load("req-1").expect("load"),
        Some(saved_row()),
        "the row restarts verbatim"
    );
    // A retried write after the reopen is a no-op, not a duplicate.
    want(reopened.save(&saved_row()), "retry after reopen");
    assert_eq!(
        reopened.load_all().expect("load all").len(),
        1,
        "the retry added no row"
    );
}

/// A reused id bound to a different scope is refused on Postgres too.
#[test]
fn postgres_a_reused_id_with_a_different_scope_is_refused() {
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "scope");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    want(store.save(&saved_row()), "first save");
    let err = store
        .save(&row("req-1", BANK_SCOPE))
        .expect_err("a reused id with a different scope must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    // The kind is frozen too: the same scope with a different kind refuses.
    let mut changed = saved_row();
    changed.kind = "export".to_owned();
    let err = store
        .save(&changed)
        .expect_err("a reused id with a different kind must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("req-1").expect("load"),
        Some(saved_row()),
        "the stored row is untouched"
    );
}

/// Two Postgres writers racing on one id leave exactly one winner: the
/// store mutex serializes the saves, so the loser always sees the winner's
/// committed row and is refused, whichever wins.
#[test]
fn postgres_two_writers_racing_on_one_id_leave_one_winner() {
    use std::sync::Arc;
    let _shared = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "race");
    drop(admin);
    let Some(store) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    let store = Arc::new(store);
    let bank = row("req-race", BANK_SCOPE);
    let subject = row("req-race", SUBJECT_SCOPE);
    let run = |row: DurableRequestRow, store: Arc<PostgresStore>| {
        std::thread::spawn(move || store.save(&row))
    };
    let bank_handle = run(bank.clone(), Arc::clone(&store));
    let subject_handle = run(subject.clone(), Arc::clone(&store));
    let bank_outcome = bank_handle.join().expect("bank writer thread");
    let subject_outcome = subject_handle.join().expect("subject writer thread");
    let (winner, loser) = match (bank_outcome, subject_outcome) {
        (Ok(()), Err(loser)) => (&bank, loser),
        (Err(loser), Ok(())) => (&subject, loser),
        _ => panic!("exactly one writer wins"),
    };
    assert_eq!(loser.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store.load("req-race").expect("load"),
        Some(winner.clone()),
        "the stored row is the winner's"
    );
}

/// The six request columns a converged table holds, in DDL order.
fn expected_columns() -> Vec<String> {
    ["request_id", "kind", "state", "scope", "deadline", "record"]
        .iter()
        .map(|column| (*column).to_owned())
        .collect()
}

/// The `durable_requests` columns in ordinal order and its index names.
fn shape(store: &PostgresStore) -> (Vec<String>, Vec<String>) {
    store
        .with_client(|client| {
            let columns = client
                .query(
                    "SELECT column_name FROM information_schema.columns
                     WHERE table_schema = current_schema()
                       AND table_name = 'durable_requests'
                     ORDER BY ordinal_position",
                    &[],
                )
                .am("durable request columns")?
                .iter()
                .map(|row| row.get::<_, String>(0))
                .collect::<Vec<_>>();
            let indexes = client
                .query(
                    "SELECT indexname FROM pg_indexes
                     WHERE schemaname = current_schema() AND tablename = 'durable_requests'",
                    &[],
                )
                .am("durable request indexes")?
                .iter()
                .map(|row| row.get::<_, String>(0))
                .collect::<Vec<_>>();
            Ok((columns, indexes))
        })
        .expect("read the durable_requests shape")
}

/// The row count of `durable_requests`.
fn row_count(store: &PostgresStore) -> i64 {
    store
        .with_client(|client| {
            client
                .query_one("SELECT count(*) FROM durable_requests", &[])
                .am("durable request row count")
                .map(|row| row.get(0))
        })
        .expect("count the durable request rows")
}

/// A schema created under the old placeholder shape converges on first use: the
/// table ends as exactly the fresh DDL's six columns with the unique index, the
/// placeholder row that carries no request is gone, and the save whose insert the
/// legacy primary key would have refused lands.
#[test]
fn postgres_a_pre_existing_placeholder_table_converges_on_first_use() {
    let _guard = crate::lock_pg_test();
    let Some(url) = shared_url() else {
        return;
    };
    let source = ScopedSchema::new(&url);
    source
        .execute("CREATE TABLE durable_requests (id text PRIMARY KEY);")
        .expect("the old placeholder shape");
    source
        .execute("INSERT INTO durable_requests VALUES ('legacy');")
        .expect("the placeholder row");
    let Some(store) = open_pg(source.url()) else {
        return;
    };
    want(store.save(&saved_row()), "save on a converged table");
    want(store.save(&saved_row()), "retry on a converged table");
    assert_eq!(
        store.load("req-1").expect("load"),
        Some(saved_row()),
        "the converged table reads back"
    );
    let (columns, indexes) = shape(&store);
    assert_eq!(
        columns,
        expected_columns(),
        "the table holds exactly the request columns, in DDL order"
    );
    assert!(
        !columns.iter().any(|name| name == "id"),
        "the legacy key is gone: {columns:?}"
    );
    assert!(
        indexes
            .iter()
            .any(|name| name == "durable_requests_request_id_uidx"),
        "the table backs the request-id index: {indexes:?}"
    );
    assert_eq!(
        row_count(&store),
        1,
        "the row that carries no request is gone and only the saved row is left"
    );

    // A fresh schema's table is the shape the repair has to reach.
    let fresh = ScopedSchema::new(&url);
    let fresh_store = fresh.open();
    assert_eq!(
        shape(&fresh_store).0,
        columns,
        "the repaired columns equal the columns a fresh database declares"
    );
}
