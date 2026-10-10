// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the derived-index rebuild sweep when
//! the embedder is optional: lexical-only success, dense fail-closed, dry-run
//! costing, confirmation/bank validation, and dimension drift.
//!
//! ## Owns
//! - `crate::rebuild::rebuild` Option-embedder paths and its validation errors
//!
//! ## Does not own
//! - Coordinator outbox behavior (`worker_tests`) or backend index SQL
//!   (`clio_store::IndexStore`)
//!
//! ## Boundary
//!
//! Tests here break when rebuild sweep policy changes; they never change it.
//!
//! Keep this module focused on rebuild Option-embedder behavior.

use crate::embed::Embedder;
use crate::embed_test_support::{FakeEmbedder, fake_vector, leaf, store};
use crate::rebuild::rebuild;
use crate::report::{RebuildReport, RebuildTargets};
use clio_store::{EMBEDDING_DIMS, IndexStore, SqliteStore, Store};
use clio_types::{AmError, ErrorCode};
use std::sync::Arc;

/// Call the free rebuild function with the store acting as both roles.
fn run(
    store: &Arc<SqliteStore>,
    bank: Option<&str>,
    embedder: Option<&dyn Embedder>,
    targets: RebuildTargets,
    dry_run: bool,
    confirm: bool,
) -> Result<RebuildReport, AmError> {
    let store_ref: &dyn Store = store.as_ref();
    let index_ref: &dyn IndexStore = store.as_ref();
    rebuild(
        store_ref, index_ref, embedder, bank, targets, dry_run, confirm,
    )
}

#[test]
fn dense_live_restores_coverage_after_wipe() {
    let store = store();
    let bank = "reb-dense";
    for i in 0..3 {
        store
            .create_memory_item(&leaf(&format!("itm-{i}"), bank, "rebuild fixture"), "actor")
            .unwrap();
    }
    let fake = FakeEmbedder::new();
    let first = run(
        &store,
        Some(bank),
        Some(&fake),
        RebuildTargets::All,
        false,
        true,
    )
    .unwrap();
    assert_eq!(first.dense_applied, 3);
    assert_eq!(first.lexical_applied, 3);

    for i in 0..3 {
        assert!(store.delete_vector(bank, &format!("itm-{i}")).unwrap());
        assert!(store.delete_lexical_doc(bank, &format!("itm-{i}")).unwrap());
    }
    assert_eq!(store.index_coverage(Some(bank)).unwrap().dense_rows, 0);

    let second = run(
        &store,
        Some(bank),
        Some(&fake),
        RebuildTargets::All,
        false,
        true,
    )
    .unwrap();
    assert_eq!(second.dense_applied, 3);
    let cov = store.index_coverage(Some(bank)).unwrap();
    assert_eq!(cov.dense_rows, 3);
    assert_eq!(cov.lexical_rows, 3);

    let query = fake_vector("rebuild fixture", EMBEDDING_DIMS);
    assert_eq!(store.dense_search(bank, &query, &[], 5).unwrap().len(), 3);
}

#[test]
fn lexical_only_without_embedder_fills_lexical() {
    let store = store();
    let bank = "reb-lex";
    for i in 0..2 {
        store
            .create_memory_item(&leaf(&format!("lex-{i}"), bank, "lexical fixture"), "actor")
            .unwrap();
    }
    let report = run(
        &store,
        Some(bank),
        None,
        RebuildTargets::Lexical,
        false,
        true,
    )
    .unwrap();
    assert_eq!(report.lexical_applied, 2);
    assert_eq!(report.dense_applied, 0);
    let cov = store.index_coverage(Some(bank)).unwrap();
    assert_eq!(cov.lexical_rows, 2);
    assert_eq!(cov.dense_rows, 0);
}

#[test]
fn dense_without_embedder_fails_closed() {
    let store = store();
    let bank = "reb-nodense";
    store
        .create_memory_item(&leaf("itm-nod", bank, "no embedder fixture"), "actor")
        .unwrap();
    for targets in [RebuildTargets::Dense, RebuildTargets::All] {
        assert_eq!(
            run(&store, Some(bank), None, targets, false, true)
                .unwrap_err()
                .code,
            ErrorCode::ConfigCorrupt
        );
    }
}

#[test]
fn dry_run_without_embedder_reports_gaps() {
    let store = store();
    let bank = "reb-dry";
    for i in 0..2 {
        store
            .create_memory_item(&leaf(&format!("dry-{i}"), bank, "dry fixture"), "actor")
            .unwrap();
    }
    let report = run(&store, Some(bank), None, RebuildTargets::All, true, false).unwrap();
    assert!(report.dry_run);
    assert_eq!(report.indexable_items, 2);
    assert_eq!(report.dense_missing, 2);
    assert_eq!(report.lexical_missing, 2);
    assert_eq!(report.dense_applied, 0);
    assert_eq!(report.lexical_applied, 0);
    assert_eq!(report.orphans_removed, 0);
}

#[test]
fn live_without_confirm_and_empty_bank_fail() {
    let store = store();
    assert_eq!(
        run(
            &store,
            Some("reb-val"),
            None,
            RebuildTargets::All,
            false,
            false
        )
        .unwrap_err()
        .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        run(&store, Some(""), None, RebuildTargets::All, false, true)
            .unwrap_err()
            .code,
        ErrorCode::MissingBank
    );
}

/// Claims the schema dimension but returns 128-wide vectors.
struct WrongDimEmbedder;

impl Embedder for WrongDimEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Ok(texts.iter().map(|t| fake_vector(t, 128)).collect())
    }

    fn health(&self) -> Result<(), AmError> {
        Ok(())
    }

    fn model_id(&self) -> String {
        "fake/wrong-dim".to_owned()
    }

    fn dims(&self) -> usize {
        EMBEDDING_DIMS
    }
}

#[test]
fn wrong_dimension_embedder_fails_closed() {
    let store = store();
    let bank = "reb-dim";
    store
        .create_memory_item(&leaf("itm-dim", bank, "dim fixture"), "actor")
        .unwrap();
    let bad = WrongDimEmbedder;
    assert_eq!(
        run(
            &store,
            Some(bank),
            Some(&bad),
            RebuildTargets::Dense,
            false,
            true
        )
        .unwrap_err()
        .code,
        ErrorCode::OutOfRange
    );
}
