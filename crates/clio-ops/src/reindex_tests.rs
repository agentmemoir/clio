// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the `reindex` operator surface:
//! dry-run costing, confirmed live rebuilds with stored-byte idempotence, and
//! fail-closed policy refusals.
//!
//! ## Owns
//! - `reindex` dry-run versus live behavior and `target_label` formatting
//! - Local embedder doubles and the seeded SQLite / Postgres store fixtures
//!
//! ## Does not own
//! - The rebuild sweep itself (`clio_index::rebuild`)
//! - Repair planning or action allowlists (`doctor` / `repair`)
//!
//! ## Boundary
//!
//! Changes to reindex UX policy should normally update tests here. Changes to
//! the rebuild sweep should not require changes here.
//!
//! Keep this module focused on reindex behavior.

use crate::reindex::{ReindexArgs, reindex, target_label};
use clio_index::{Embedder, RebuildTargets};
use clio_store::{EMBEDDING_DIMS, IndexStore, ScopedSchema, SqliteStore, Store, lock_pg_test};
use clio_types::{AmError, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem};
use serde_json::json;
use std::sync::Arc;

const TS: &str = "2026-01-01T00:00:00Z";

fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(clio_store::LocalDevKms::new())).expect("in-memory store")
}

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn_1".to_owned()),
        snapshot: Some(json!({"error_code": "ECONNRESET"})),
        gist: Some("deploy fixture".to_owned()),
        valid_from: TS.to_owned(),
        valid_until: None,
        tx_from: TS.to_owned(),
        tx_until: None,
        created_at: TS.to_owned(),
        updated_at: TS.to_owned(),
        context: None,
    }
}

struct FakeEmbedder;

impl Embedder for FakeEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Ok(texts
            .iter()
            .map(|_| vec![0.0_f32; EMBEDDING_DIMS])
            .collect())
    }

    fn health(&self) -> Result<(), AmError> {
        Ok(())
    }

    fn model_id(&self) -> String {
        clio_store::DEFAULT_EMBEDDING_MODEL.to_owned()
    }

    fn dims(&self) -> usize {
        EMBEDDING_DIMS
    }
}

#[test]
fn target_label_covers_every_target() {
    assert_eq!(target_label(RebuildTargets::Dense), "dense");
    assert_eq!(target_label(RebuildTargets::Lexical), "lexical");
    assert_eq!(target_label(RebuildTargets::All), "all");
}

#[test]
fn dry_run_reports_counts_without_writing() {
    let store = store();
    store.create_memory_item(&item("i1", "b"), "actor").unwrap();
    store.create_memory_item(&item("i2", "b"), "actor").unwrap();
    let report = reindex(&ReindexArgs {
        store: &store,
        index: &store,
        embedder: None,
        bank: Some("b"),
        target: RebuildTargets::All,
        dry_run: true,
        confirm: false,
    })
    .unwrap();
    assert!(
        !report.ok,
        "a dry run applies nothing, so a no-op report is never ok"
    );
    assert!(report.dry_run);
    assert_eq!(report.target, "all");
    assert_eq!(report.bank.as_deref(), Some("b"));
    assert_eq!(report.indexable_items, 2);
    assert_eq!(report.dense_missing, 2);
    assert_eq!(report.lexical_missing, 2);
    assert_eq!(report.dense_applied, 0);
    assert_eq!(report.lexical_applied, 0);
    let cov = clio_store::IndexStore::index_coverage(&store, Some("b")).unwrap();
    assert_eq!(cov.dense_rows, 0);
    assert_eq!(cov.lexical_rows, 0);
}

#[test]
fn live_lexical_rebuild_needs_no_embedder() {
    let store = store();
    store.create_memory_item(&item("i1", "b"), "actor").unwrap();
    let report = reindex(&ReindexArgs {
        store: &store,
        index: &store,
        embedder: None,
        bank: Some("b"),
        target: RebuildTargets::Lexical,
        dry_run: false,
        confirm: true,
    })
    .unwrap();
    assert!(report.ok);
    assert!(!report.dry_run);
    assert_eq!(report.lexical_applied, 1);
    assert_eq!(report.dense_applied, 0);
}

#[test]
fn live_dense_rebuild_with_embedder_writes_rows() {
    let store = store();
    store.create_memory_item(&item("i1", "b"), "actor").unwrap();
    let embedder = FakeEmbedder;
    let report = reindex(&ReindexArgs {
        store: &store,
        index: &store,
        embedder: Some(&embedder),
        bank: Some("b"),
        target: RebuildTargets::Dense,
        dry_run: false,
        confirm: true,
    })
    .unwrap();
    assert_eq!(report.dense_applied, 1);
    let cov = clio_store::IndexStore::index_coverage(&store, Some("b")).unwrap();
    assert_eq!(cov.dense_rows, 1);
}

#[test]
fn mutating_without_confirm_fails_closed() {
    let store = store();
    let err = reindex(&ReindexArgs {
        store: &store,
        index: &store,
        embedder: None,
        bank: Some("b"),
        target: RebuildTargets::Lexical,
        dry_run: false,
        confirm: false,
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

/// A dry run reports and a confirmed run applies, so asking for both is a
/// contradiction rather than a request the report can honor.
#[test]
fn confirm_with_a_dry_run_is_refused() {
    let store = store();
    store.create_memory_item(&item("i1", "b"), "actor").unwrap();
    let err = reindex(&ReindexArgs {
        store: &store,
        index: &store,
        embedder: None,
        bank: Some("b"),
        target: RebuildTargets::Lexical,
        dry_run: true,
        confirm: true,
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let cov = clio_store::IndexStore::index_coverage(&store, Some("b")).unwrap();
    assert_eq!(cov.lexical_rows, 0, "the refused call wrote nothing");
}

#[test]
fn live_dense_without_embedder_fails_closed() {
    let store = store();
    let err = reindex(&ReindexArgs {
        store: &store,
        index: &store,
        embedder: None,
        bank: Some("b"),
        target: RebuildTargets::Dense,
        dry_run: false,
        confirm: true,
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}

#[test]
fn empty_bank_is_missing_bank() {
    let store = store();
    let err = reindex(&ReindexArgs {
        store: &store,
        index: &store,
        embedder: None,
        bank: Some(""),
        target: RebuildTargets::All,
        dry_run: true,
        confirm: false,
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);
}

/// The raw `tsvector` renderings of every item row in one bank, id-ordered.
fn stored_rows(schema: &ScopedSchema, bank: &str) -> String {
    schema
        .query_value(&format!(
            "SELECT COALESCE(string_agg(content_tsv::text, '|' ORDER BY id), '')
             FROM items WHERE bank_id = '{bank}'"
        ))
        .expect("read stored tsvectors")
        .unwrap_or_default()
}

/// A live lexical reindex rewrites every item row (`lexical_applied` equals the
/// indexable count), and a second run stores identical bytes.
///
/// A private Postgres schema lets this test read the raw stored column back
/// through the scoped-schema SQL helpers; `content_tsv::text` is the stored
/// value's own serialization, so comparing it across runs is exact.
#[test]
fn live_lexical_reindex_rewrites_every_row_and_repeats_identically() {
    let _guard = lock_pg_test();
    let base = std::env::var("CLIO_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://clio:clio@127.0.0.1:34310/clio".to_owned());
    let schema = ScopedSchema::new(&base);
    let store = schema.open();
    let bank = "b";
    let texts = [
        "zephyr deployment note",
        "quasar rollout plan",
        "nebula backup job",
    ];
    for (index, text) in texts.iter().enumerate() {
        let id = format!("item-{index}");
        let mut record = item(&id, bank);
        record.gist = Some((*text).to_owned());
        store
            .create_memory_item(&record, "actor")
            .expect("create item");
        // Seed the legacy plaintext shape before the reindex runs.
        schema
            .execute(&format!(
                "UPDATE items SET content_tsv = to_tsvector('simple', '{text}')
                 WHERE id = '{id}' AND bank_id = '{bank}'"
            ))
            .expect("seed old-format tsvector");
    }

    let report = reindex(&ReindexArgs {
        store: &store,
        index: &store,
        embedder: None,
        bank: Some(bank),
        target: RebuildTargets::Lexical,
        dry_run: false,
        confirm: true,
    })
    .expect("live lexical reindex");
    assert!(report.ok);
    assert_eq!(report.indexable_items, 3);
    assert_eq!(
        report.lexical_applied, report.indexable_items,
        "every item row is rewritten"
    );
    let cov = store.index_coverage(Some(bank)).expect("coverage");
    assert_eq!(cov.indexable_items, 3);
    assert_eq!(cov.lexical_rows, 3);
    let first = stored_rows(&schema, bank);
    for word in ["zephyr", "quasar", "nebula"] {
        assert!(!first.contains(word), "the word `{word}` survives: {first}");
    }

    let again = reindex(&ReindexArgs {
        store: &store,
        index: &store,
        embedder: None,
        bank: Some(bank),
        target: RebuildTargets::Lexical,
        dry_run: false,
        confirm: true,
    })
    .expect("second live lexical reindex");
    assert_eq!(again.lexical_applied, again.indexable_items);
    assert_eq!(
        stored_rows(&schema, bank),
        first,
        "a repeated reindex changes no stored bytes"
    );
}
