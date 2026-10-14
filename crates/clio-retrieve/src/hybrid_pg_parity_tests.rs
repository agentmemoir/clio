// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the opt-in dual-backend parity check of the
//! hybrid retrieval orchestrator over SQLite and Postgres.
//!
//! ## Owns
//! - `postgres_and_sqlite_hit_id_parity` (same hit ids on both backends)
//!
//! ## Does not own
//! - Backend search SQL or ranking (owned by `clio-store`)
//! - SQLite-only behavior assertions (owned by the other `*_tests` modules)
//!
//! ## Boundary
//!
//! Opt-in: runs only when `CLIO_PG_RETRIEVE_PARITY=1` and `CLIO_DATABASE_URL` are set,
//! so it never contends with the serialized Postgres suites during normal
//! `cargo test`/coverage runs.

use crate::fixtures_tests as fx;
use crate::hybrid::HybridRetriever;
use crate::types::RetrieveRequest;
use clio_store::{DekProvider, IndexStore, LocalDevKms, PostgresStore, Store, lock_pg_test};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process, so `unique` never repeats.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique(prefix: &str) -> String {
    // The counter is what makes this distinct: the process id is constant
    // inside one process, so clock-plus-pid alone repeats whenever two calls
    // land in the same tick, and this name is a bank id other tests share.
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{prefix}-{nanos}-{}-{sequence}", std::process::id())
}

fn seed<S: Store + IndexStore>(store: &S, bank: &str) {
    for (id, gist) in [("A", "alpha shared token"), ("B", "beta shared token")] {
        let entry = fx::item(id, bank, gist);
        store.create_memory_item(&entry, "agent").expect("create");
        store.put_lexical_doc(bank, id, gist).expect("lexical");
    }
}

fn sorted_ids(out: &crate::types::RetrieveOutcome) -> Vec<String> {
    let mut ids: Vec<String> = out.hits.iter().map(|h| h.hit.item_id.clone()).collect();
    ids.sort();
    ids
}

#[test]
fn postgres_and_sqlite_hit_id_parity() {
    let Ok(url) = std::env::var("CLIO_DATABASE_URL") else {
        eprintln!("skip postgres retrieve parity: CLIO_DATABASE_URL unset");
        return;
    };
    if std::env::var("CLIO_PG_RETRIEVE_PARITY").as_deref() != Ok("1") {
        eprintln!("skip postgres retrieve parity: set CLIO_PG_RETRIEVE_PARITY=1 to run");
        return;
    }
    let _guard = lock_pg_test();
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let Ok(pg) = PostgresStore::open(&url, kms) else {
        eprintln!("skip postgres retrieve parity: cannot open {url}");
        return;
    };
    let bank = unique("retrieve-parity");
    let pg = Arc::new(pg);
    let sqlite = fx::store();
    seed(pg.as_ref(), &bank);
    seed(sqlite.as_ref(), &bank);

    let pg_retriever = HybridRetriever::new(
        Arc::clone(&pg) as Arc<dyn Store>,
        Arc::clone(&pg) as Arc<dyn IndexStore>,
    );
    let sqlite_retriever = fx::retriever(&sqlite);
    let req = RetrieveRequest::new(&bank, "shared");
    let pg_out = pg_retriever.retrieve(&req).expect("pg retrieve");
    let sqlite_out = sqlite_retriever.retrieve(&req).expect("sqlite retrieve");
    assert_eq!(sorted_ids(&pg_out), sorted_ids(&sqlite_out));
    assert!(!pg_out.hits.is_empty(), "{:?}", pg_out.hits);
}
