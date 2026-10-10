// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the rebuild hooks and their coverage
//! recovery behavior.
//!
//! ## Owns
//! - Dry-run versus confirmed apply, wipe recovery, orphan cleanup, paging, and scope selection
//!
//! ## Does not own
//! - Outbox worker retry behavior (see `worker_tests`)
//! - Backend index SQL (`clio_store::IndexStore`)
//!
//! ## Boundary
//!
//! Changes to rebuild policy should normally update tests here.
//! Changes to backend SQL should not require changes here.
//!
//! Keep this module focused on rebuild behavior.

use crate::embed_test_support::{FakeEmbedder, coordinator, fake_vector, leaf, store};
use crate::report::RebuildTargets;
use clio_store::{EMBEDDING_DIMS, IndexStore, Store, VectorSlot};
use std::sync::Arc;

/// Rebuild dry-run counts only; confirmed apply restores coverage after an
/// intentional wipe.
#[test]
fn t06_t07_rebuild_dry_run_then_apply_restores_wiped_coverage() {
    let store = store();
    let coordinator = coordinator(&store, Arc::new(FakeEmbedder::new()));
    let bank = "t07";
    for i in 0..3 {
        store
            .create_memory_item(&leaf(&format!("itm-{i}"), bank, "rebuild fixture"), "actor")
            .unwrap();
    }
    coordinator.process_pending(10).unwrap();
    assert_eq!(store.index_coverage(Some(bank)).unwrap().dense_rows, 3);

    // Intentional wipe of derived rows (rollback strategy: derived state only).
    for i in 0..3 {
        assert!(store.delete_vector(bank, &format!("itm-{i}")).unwrap());
        assert!(store.delete_lexical_doc(bank, &format!("itm-{i}")).unwrap());
    }
    assert_eq!(store.index_coverage(Some(bank)).unwrap().dense_rows, 0);

    // Dry-run reports counts without mutation.
    let dry = coordinator
        .rebuild_indexes(Some(bank), RebuildTargets::All, true, false)
        .unwrap();
    assert!(dry.dry_run);
    assert_eq!(dry.indexable_items, 3);
    assert_eq!(dry.dense_missing, 3);
    assert_eq!(dry.dense_applied, 0);
    assert_eq!(store.index_coverage(Some(bank)).unwrap().dense_rows, 0);

    // Mutating rebuild without confirm fails closed.
    assert!(
        coordinator
            .rebuild_indexes(Some(bank), RebuildTargets::All, false, false)
            .is_err()
    );

    // Confirmed apply restores coverage.
    let apply = coordinator
        .rebuild_indexes(Some(bank), RebuildTargets::All, false, true)
        .unwrap();
    assert_eq!(apply.dense_applied, 3);
    assert_eq!(apply.lexical_applied, 3);
    let cov = store.index_coverage(Some(bank)).unwrap();
    assert_eq!(cov.dense_rows, 3);
    assert_eq!(cov.lexical_rows, 3);
    // Search works again after rebuild.
    let query = fake_vector("rebuild fixture", EMBEDDING_DIMS);
    let dense = clio_store::IndexStore::dense_search(&*store, bank, &query, &[], 5).unwrap();
    assert_eq!(dense.len(), 3);

    // Whole-store scope + targets parsing.
    let parsed = RebuildTargets::parse("dense").unwrap();
    assert_eq!(parsed, RebuildTargets::Dense);
    assert!(RebuildTargets::parse("nope").is_err());
}

/// Rebuild clears orphans and reports the removal.
#[test]
fn rebuild_removes_orphan_derived_rows() {
    let store = store();
    let coordinator = coordinator(&store, Arc::new(FakeEmbedder::new()));
    let bank = "orph";
    store
        .create_memory_item(&leaf("itm-orphan", bank, "orphan fixture"), "actor")
        .unwrap();
    coordinator.process_pending(10).unwrap();
    let mut emb = vec![0.0_f32; EMBEDDING_DIMS];
    emb[3] = 1.0;
    store
        .upsert_vector(&VectorSlot {
            item_id: "gone-item".into(),
            bank_id: bank.into(),
            model_id: clio_store::DEFAULT_EMBEDDING_MODEL.into(),
            embedding: emb.clone(),
        })
        .unwrap();
    assert!(coordinator.enqueue_leaf(bank, "gone-item").unwrap());
    let apply = coordinator
        .rebuild_indexes(Some(bank), RebuildTargets::Dense, false, true)
        .unwrap();
    assert!(
        apply.orphans_removed >= 2,
        "vector + queue rows for gone-item: {apply:?}"
    );
    // No dense rows remain for vanished items; visible coverage intact.
    assert_eq!(store.index_coverage(Some(bank)).unwrap().dense_rows, 1);
}

/// Rebuild sweeps pages when the batch size is tiny (offset pagination works).
#[test]
fn rebuild_pages_through_many_items() {
    let store = store();
    let coordinator = coordinator(&store, Arc::new(FakeEmbedder::new()));
    let bank = "page";
    for i in 0..5 {
        store
            .create_memory_item(&leaf(&format!("itm-p{i}"), bank, "page fixture"), "actor")
            .unwrap();
    }
    let apply = coordinator
        .rebuild_indexes(Some(bank), RebuildTargets::Dense, false, true)
        .unwrap();
    assert_eq!(apply.dense_applied, 5);
    assert_eq!(store.index_coverage(Some(bank)).unwrap().dense_rows, 5);
}

/// Whole-store rebuild discovers scopes, lexical-only targets work, and
/// empty banks fail closed.
#[test]
fn rebuild_scope_and_target_variants() {
    let store = store();
    let coordinator = coordinator(&store, Arc::new(FakeEmbedder::new()));
    let bank = "scope";
    store
        .create_memory_item(&leaf("itm-s1", bank, "scope fixture"), "actor")
        .unwrap();
    store
        .create_memory_item(&leaf("itm-s2", bank, "scope fixture"), "actor")
        .unwrap();
    // Empty bank fails closed.
    assert_eq!(
        coordinator
            .rebuild_indexes(Some(""), RebuildTargets::All, false, true)
            .unwrap_err()
            .code,
        clio_types::ErrorCode::MissingBank
    );
    // Lexical-only rebuild fills only lexical rows.
    let lexical_only = coordinator
        .rebuild_indexes(Some(bank), RebuildTargets::Lexical, false, true)
        .unwrap();
    assert_eq!(lexical_only.lexical_applied, 2);
    assert_eq!(lexical_only.dense_applied, 0);
    let cov = store.index_coverage(Some(bank)).unwrap();
    assert_eq!(cov.lexical_rows, 2);
    assert_eq!(cov.dense_rows, 0);
    // Whole-store rebuild (bank=None) sweeps every bank's gaps.
    let all = coordinator
        .rebuild_indexes(None, RebuildTargets::All, false, true)
        .unwrap();
    assert!(all.indexable_items >= 2);
    assert_eq!(all.dense_applied, 2);
    let cov = store.index_coverage(None).unwrap();
    assert_eq!(cov.dense_rows, 2);
}
