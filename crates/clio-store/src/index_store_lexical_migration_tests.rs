// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the migration evidence of the lexical index:
//! an old-format store ends in the keyed form, a repeated rewrite is a no-op,
//! a partial rewrite leaves no plaintext, and coverage still counts the rows.
//!
//! ## Owns
//! - Old-format-to-keyed-form rewrite tests for the SQLite and Postgres stores
//! - Determinism (twice is a no-op) and partial-rewrite tests
//!
//! ## Does not own
//! - The keyed encoding (`lexical_digest`) and the rebuild sweep (`clio-ops` /
//!   `clio-index` reindex tests)
//!
//! ## Boundary
//!
//! Tests here break when the stored lexical form or the rewrite path changes;
//! they never change either.
//!
//! Keep this module focused on the migration evidence.

use crate::dek::LocalDevKms;
use crate::index_store::{IndexStore, encode_lexical_document};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem};
use rusqlite::params;
use serde_json::json;
use std::sync::Arc;

const TS: &str = "2026-01-01T00:00:00Z";

fn item(id: &str, bank: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.5),
        source_ref: Some("turn_1".into()),
        snapshot: Some(json!({"error_code": "ECONNRESET", "service": "deploy-bot"})),
        gist: Some(gist.into()),
        valid_from: TS.into(),
        valid_until: None,
        tx_from: TS.into(),
        tx_until: None,
        created_at: TS.into(),
        updated_at: TS.into(),
        context: None,
    }
}

/// One in-memory SQLite store with its own fresh key material.
fn sqlite_store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("in-memory sqlite store")
}

/// One store over the shared test database with fresh key material.
fn postgres_store() -> PostgresStore {
    let url = crate::db_url::postgres_url_from_env();
    PostgresStore::open(url.as_str(), Arc::new(LocalDevKms::new())).expect("postgres store")
}

/// One stored term must be the letter prefix plus a lowercase hex digest,
/// never a readable word.
fn is_digest_term(term: &str) -> bool {
    term.len() == crate::lexical_digest::DIGEST_TERM_LEN
        && term.starts_with('k')
        && term[1..]
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// The whole stored lexical value must be space-joined keyed digests.
fn is_digest_row(stored: &str) -> bool {
    !stored.is_empty() && stored.split_whitespace().all(is_digest_term)
}

/// Seed one old-format (plaintext) FTS5 row, the shape a legacy store holds.
fn seed_plaintext_sqlite(store: &SqliteStore, bank: &str, id: &str, text: &str) {
    store
        .with_conn(|conn| {
            conn.execute(
                "DELETE FROM items_fts WHERE item_id = ?1 AND bank_id = ?2",
                params![id, bank],
            )
            .am("old-format seed delete")?;
            conn.execute(
                "INSERT INTO items_fts (item_id, bank_id, content_text) VALUES (?1, ?2, ?3)",
                params![id, bank, text],
            )
            .am("old-format seed insert")
        })
        .expect("seed old-format fts row");
}

/// The raw FTS5 stored value for one item.
fn raw_sqlite(store: &SqliteStore, bank: &str, id: &str) -> String {
    store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT content_text FROM items_fts WHERE item_id = ?1 AND bank_id = ?2",
                params![id, bank],
                |row| row.get(0),
            )
            .am("raw fts read")
        })
        .expect("raw fts row exists")
}

/// Create one item per text and seed each with its old-format plaintext row.
fn seed_old_format_rows(store: &SqliteStore, bank: &str, texts: &[&str]) -> Vec<String> {
    let mut ids = Vec::with_capacity(texts.len());
    for (index, text) in texts.iter().enumerate() {
        let id = unique(&format!("migrate-item-{index}"));
        store
            .create_memory_item(&item(&id, bank, text), "actor")
            .expect("create item");
        seed_plaintext_sqlite(store, bank, &id, text);
        ids.push(id);
    }
    ids
}

/// Seed one old-format plaintext `content_tsv`, the shape a legacy store holds.
fn seed_plaintext_pg(store: &PostgresStore, bank: &str, id: &str, text: &str) {
    store
        .with_client(|client| {
            client
                .execute(
                    "UPDATE items SET content_tsv = to_tsvector('simple', $3)
                     WHERE id = $1 AND bank_id = $2",
                    &[&id, &bank, &text],
                )
                .am("old-format seed")
                .map(|_| ())
        })
        .expect("seed old-format tsvector");
}

/// The raw `tsvector` rendering for one item.
fn raw_pg(store: &PostgresStore, bank: &str, id: &str) -> String {
    store
        .with_client(|client| {
            client
                .query_one(
                    "SELECT content_tsv::text FROM items WHERE id = $1 AND bank_id = $2",
                    &[&id, &bank],
                )
                .am("raw tsvector read")
                .map(|row| row.get::<_, String>(0))
        })
        .expect("raw tsvector row exists")
}

/// The individual lexemes of one item's stored `tsvector`.
fn lexemes_pg(store: &PostgresStore, bank: &str, id: &str) -> Vec<String> {
    store
        .with_client(|client| {
            let rows = client
                .query(
                    "SELECT t.lexeme
                     FROM items, unnest(items.content_tsv) AS t(lexeme, positions)
                     WHERE items.id = $1 AND items.bank_id = $2
                     ORDER BY t.lexeme",
                    &[&id, &bank],
                )
                .am("lexeme read")?;
            Ok(rows
                .into_iter()
                .map(|row| row.get::<_, String>(0))
                .collect())
        })
        .expect("lexemes read")
}

/// The raw `tsvector` renderings of every item row in one bank, id-ordered.
fn raw_bank_pg(store: &PostgresStore, bank: &str) -> String {
    store
        .with_client(|client| {
            client
                .query_one(
                    "SELECT COALESCE(string_agg(content_tsv::text, '|' ORDER BY id), '')
                     FROM items WHERE bank_id = $1",
                    &[&bank],
                )
                .am("bank tsvector read")
                .map(|row| row.get::<_, String>(0))
        })
        .expect("bank tsvector read")
}

#[test]
fn sqlite_old_plaintext_row_is_replaced_by_digests() {
    let store = sqlite_store();
    let bank = unique("sqlite-migrate-bank");
    let id = unique("sqlite-migrate-item");
    let text = "Deploying the zephyr vault failed with ECONNRESET";
    store
        .create_memory_item(&item(&id, &bank, text), "actor")
        .expect("create item");
    seed_plaintext_sqlite(&store, &bank, &id, text);
    assert_eq!(
        raw_sqlite(&store, &bank, &id),
        text,
        "precondition: the legacy row holds plaintext"
    );

    store
        .put_lexical_doc(&bank, &id, text)
        .expect("rewrite the legacy row");

    let raw = raw_sqlite(&store, &bank, &id);
    assert!(is_digest_row(&raw), "stored form is digests only: {raw}");
    assert_eq!(
        raw,
        encode_lexical_document(store.kms().as_ref(), text).expect("encode"),
        "the rewrite stores exactly the keyed encoding"
    );
    assert!(!raw.contains("zephyr"), "still no plaintext word");
    let hits = store
        .lexical_search(&bank, "zephyr", &[], 5)
        .expect("search after rewrite");
    assert_eq!(hits.first().expect("a hit").item_id, id);
}

#[test]
fn sqlite_second_rewrite_is_byte_identical_and_coverage_holds() {
    let store = sqlite_store();
    let bank = unique("sqlite-repeat-bank");
    let texts = ["zephyr deployment note", "quasar rollout plan"];
    let ids = seed_old_format_rows(&store, &bank, &texts);

    let before = store.index_coverage(Some(&bank)).expect("coverage before");
    assert_eq!(before.indexable_items, 2);
    assert_eq!(
        before.lexical_rows, 2,
        "old-format rows count as rows present"
    );

    for (id, text) in ids.iter().zip(texts) {
        store
            .put_lexical_doc(&bank, id, text)
            .expect("first rewrite");
    }
    let first: Vec<String> = ids.iter().map(|id| raw_sqlite(&store, &bank, id)).collect();
    for raw in &first {
        assert!(is_digest_row(raw), "first rewrite stores digests: {raw}");
    }

    for (id, text) in ids.iter().zip(texts) {
        store
            .put_lexical_doc(&bank, id, text)
            .expect("second rewrite");
    }
    let second: Vec<String> = ids.iter().map(|id| raw_sqlite(&store, &bank, id)).collect();
    assert_eq!(first, second, "a repeated rewrite changes no stored bytes");

    let after = store.index_coverage(Some(&bank)).expect("coverage after");
    assert_eq!(
        after, before,
        "coverage is identical after the repeated rewrite"
    );
}

#[test]
fn sqlite_coverage_counts_exactly_the_rows_present() {
    let store = sqlite_store();
    let bank = unique("sqlite-coverage-bank");
    let texts = ["zephyr deployment note", "quasar rollout plan"];
    let mut ids = Vec::new();
    for text in texts {
        let id = unique("sqlite-coverage-item");
        store
            .create_memory_item(&item(&id, &bank, text), "actor")
            .expect("create item");
        ids.push(id);
    }

    let empty = store.index_coverage(Some(&bank)).expect("coverage");
    assert_eq!(empty.indexable_items, 2);
    assert_eq!(empty.lexical_rows, 0, "no lexical row exists yet");

    seed_plaintext_sqlite(&store, &bank, &ids[0], texts[0]);
    let one = store.index_coverage(Some(&bank)).expect("coverage");
    assert_eq!(one.lexical_rows, 1, "counts the one row present");

    store
        .put_lexical_doc(&bank, &ids[0], texts[0])
        .expect("rewrite one row");
    let still = store.index_coverage(Some(&bank)).expect("coverage");
    assert_eq!(still.lexical_rows, 1, "count unchanged");

    store
        .put_lexical_doc(&bank, &ids[1], texts[1])
        .expect("rewrite the rest");
    let full = store.index_coverage(Some(&bank)).expect("coverage");
    assert_eq!(full.indexable_items, 2);
    assert_eq!(full.lexical_rows, 2);
}

#[test]
fn sqlite_partial_rewrite_writes_no_plaintext() {
    let store = sqlite_store();
    let bank = unique("sqlite-partial-bank");
    let text_a = "zephyr deployment zephyr";
    let text_b = "quasar rollout quasar";
    let ids = seed_old_format_rows(&store, &bank, &[text_a, text_b]);

    // A sweep interrupted after the first row: one row is keyed, one is untouched.
    store
        .put_lexical_doc(&bank, &ids[0], text_a)
        .expect("rewrite the first row");

    let raw_a = raw_sqlite(&store, &bank, &ids[0]);
    assert!(is_digest_row(&raw_a), "the touched row is digests: {raw_a}");
    assert!(!raw_a.contains("zephyr"), "a touched row holds no word");
    let raw_b = raw_sqlite(&store, &bank, &ids[1]);
    assert_eq!(raw_b, text_b, "the untouched row keeps its old plaintext");

    // Every row is either its old plaintext or digests; no third form appears.
    for (id, text) in ids.iter().zip([text_a, text_b]) {
        let raw = raw_sqlite(&store, &bank, id);
        assert!(
            raw == text || is_digest_row(&raw),
            "no intermediate stored form: {raw}"
        );
    }
    let cov = store.index_coverage(Some(&bank)).expect("coverage");
    assert_eq!(cov.indexable_items, 2);
    assert_eq!(cov.lexical_rows, 2);

    // The retry completes the interrupted sweep without writing plaintext.
    store
        .put_lexical_doc(&bank, &ids[1], text_b)
        .expect("retry the second row");
    let raw_b_after = raw_sqlite(&store, &bank, &ids[1]);
    assert!(is_digest_row(&raw_b_after), "retried row is digests");
    assert!(!raw_b_after.contains("quasar"));
}

#[test]
fn postgres_old_plaintext_row_is_replaced_by_digests() {
    let _guard = crate::lock_pg_test();
    let store = postgres_store();
    let bank = unique("pg-migrate-bank");
    let id = unique("pg-migrate-item");
    let text = "Deploying the zephyr vault failed with ECONNRESET";
    store
        .create_memory_item(&item(&id, &bank, text), "actor")
        .expect("create item");
    seed_plaintext_pg(&store, &bank, &id, text);
    assert!(
        raw_pg(&store, &bank, &id).contains("zephyr"),
        "precondition: the legacy row holds plaintext"
    );

    store
        .put_lexical_doc(&bank, &id, text)
        .expect("rewrite the legacy row");

    let raw = raw_pg(&store, &bank, &id);
    assert!(!raw.contains("zephyr"), "no readable word survives: {raw}");
    let lexemes = lexemes_pg(&store, &bank, &id);
    assert!(!lexemes.is_empty(), "the rewritten row carries keyed terms");
    for lexeme in &lexemes {
        assert!(is_digest_term(lexeme), "non-digest lexeme `{lexeme}`");
    }
    let cov = store.index_coverage(Some(&bank)).expect("coverage");
    assert_eq!(cov.indexable_items, 1);
    assert_eq!(cov.lexical_rows, 1);
}

#[test]
fn postgres_partial_and_repeated_rewrites_leave_no_plaintext() {
    let _guard = crate::lock_pg_test();
    let store = postgres_store();
    let bank = unique("pg-repeat-bank");
    let id_a = unique("pg-repeat-item-a");
    let id_b = unique("pg-repeat-item-b");
    let (text_a, text_b) = ("zephyr deployment note", "quasar rollout plan");
    store
        .create_memory_item(&item(&id_a, &bank, text_a), "actor")
        .expect("create item a");
    store
        .create_memory_item(&item(&id_b, &bank, text_b), "actor")
        .expect("create item b");
    seed_plaintext_pg(&store, &bank, &id_a, text_a);
    seed_plaintext_pg(&store, &bank, &id_b, text_b);

    // A sweep interrupted after the first row: one row is keyed, one is untouched.
    store
        .put_lexical_doc(&bank, &id_a, text_a)
        .expect("rewrite the first row");
    let partial = raw_bank_pg(&store, &bank);
    assert!(
        !partial.contains("zephyr"),
        "a touched row holds no word: {partial}"
    );
    assert!(
        partial.contains("quasar"),
        "the untouched row keeps its old plaintext"
    );
    let partial_cov = store.index_coverage(Some(&bank)).expect("coverage");
    assert_eq!(partial_cov.indexable_items, 2);
    assert_eq!(
        partial_cov.lexical_rows, 2,
        "coverage counts both rows present"
    );

    // The retry completes the sweep; a second full pass changes no bytes.
    store
        .put_lexical_doc(&bank, &id_b, text_b)
        .expect("retry the second row");
    let first = raw_bank_pg(&store, &bank);
    assert!(
        !first.contains("zephyr") && !first.contains("quasar"),
        "{first}"
    );
    store
        .put_lexical_doc(&bank, &id_a, text_a)
        .expect("repeat the first row");
    store
        .put_lexical_doc(&bank, &id_b, text_b)
        .expect("repeat the second row");
    let second = raw_bank_pg(&store, &bank);
    assert_eq!(first, second, "a repeated rewrite changes no stored bytes");

    let cov = store.index_coverage(Some(&bank)).expect("coverage");
    assert_eq!(cov.indexable_items, 2);
    assert_eq!(cov.lexical_rows, 2);
}
