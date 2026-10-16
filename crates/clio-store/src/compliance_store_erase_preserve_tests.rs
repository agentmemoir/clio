// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the erasure-preservation store tests on real
//! SQLite: the other subject keeps its items and its key, a derivative shared
//! with the erased subject stops exposing erased content, a later ordinary
//! write and an import batch for the erased subject are refused without
//! recreating its key, and a key destroyed before the erasure stays destroyed
//! through the SQL work and a retry.
//!
//! ## Owns
//! - The two-subject, shared-derivative, key-recreation, and pre-destroyed-key
//!   erasure scenarios
//!
//! ## Does not own
//! - Test fixtures and shared assertions (`erase_test_support`)
//! - The erasure transaction under test (`sqlite_erase` / `sqlite_erase_purge`)
//!
//! ## Boundary
//!
//! Changes to what one subject's erasure preserves for another should normally
//! require changes here. Changes to fixture shape should require
//! `erase_test_support`.
//!
//! Keep this module focused on erasure preservation scenarios.
//!
//! Test-only. Never compiled into production builds.

use crate::batch_model::{BatchStore, BatchWrite};
use crate::compliance_store::{ComplianceStore, SubjectEraseOutcome};
use crate::dek::DekProvider;
use crate::distilled_unit::DistilledUnitRecord;
use crate::erase_test_support::{
    assert_erased, link_hub, local_kms, memory_item, seed, spec, unique,
};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::sync_store::SyncStore;
use clio_types::ErrorCode;
use rusqlite::params;
use std::sync::Arc;

/// Every `subject_keys` row for one subject as `(status, dek_ref, shredded_at)`,
/// so a test can compare the key rows of the store before and after an
/// attempted write.
fn subject_key_rows(store: &SqliteStore, subject: &str) -> Vec<(String, String, Option<String>)> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT status, dek_ref, shredded_at FROM subject_keys
                     WHERE subject_id = ?1 ORDER BY status, dek_ref",
                )
                .am("subject key rows prepare")?;
            let rows = stmt
                .query_map(params![subject], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .am("subject key rows query")?;
            rows.collect::<Result<Vec<_>, _>>().am("subject key rows")
        })
        .expect("subject key rows readable")
}

/// The stored `content_ciphertext` of one item row as-is, or `None` when it was
/// cleared, so a test can show that no sealed bytes survive in the row.
fn stored_ciphertext(store: &SqliteStore, item_id: &str) -> Option<String> {
    store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT content_ciphertext FROM items WHERE id = ?1",
                params![item_id],
                |r| r.get(0),
            )
            .am("read stored ciphertext")
        })
        .expect("the content column is readable")
}

/// A hub-distilled derivative claiming both subjects' items, seeded through the
/// real distilled-unit write path: `commit_distilled_unit` plus one
/// `hub_distill` cross-reference edge per claimed item (the edges `link_hub`
/// writes). The derivative's own content is sealed under the hub subject's key,
/// so the erasure of a claimed source can only purge it, never decrypt it.
fn seed_shared_derivative(
    store: &dyn Store,
    bank: &str,
    hub_subject: &str,
    item_a: &str,
    item_b: &str,
) -> String {
    let hub = unique("hub");
    let derivative = format!("hub-distill:{bank}:{hub}");
    let mut item = memory_item(bank, &derivative, hub_subject);
    item.source_ref = Some("hub_distilled".into());
    item.snapshot = Some(serde_json::json!({
        "hub_id": hub,
        "neighbor_ids": [item_a, item_b],
        "neighbor_count": 2,
    }));
    let record = DistilledUnitRecord::new(
        bank,
        &derivative,
        &hub,
        [item_a.to_owned(), item_b.to_owned()],
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    store
        .commit_distilled_unit(&item, &record, "tester")
        .expect("seed the shared derivative");
    link_hub(store, bank, &derivative, item_a);
    link_hub(store, bank, &derivative, item_b);
    derivative
}

/// One erased subject beside a surviving one: the store, the bank, the
/// survivor's item id, and the survivor's key rows as the erasure left them.
struct ErasedBesideSurvivor {
    store: SqliteStore,
    bank: String,
    subject_b: String,
    item_b: String,
    key_rows_b: Vec<(String, String, Option<String>)>,
}

/// Erase one subject of a two-subject bank and return the surviving side: the
/// setup both preservation claims share. The erasure's own effect is pinned
/// here, so neither half can pass over an erasure that never ran.
fn erased_beside_a_survivor() -> ErasedBesideSurvivor {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("preserve-bank");
    let subject_a = unique("subject-a");
    let subject_b = unique("subject-b");
    let (item_a, parent) = seed(&store, &bank, &subject_a);
    let item_b = unique("item-b");
    let mut survivor_item = memory_item(&bank, &item_b, &subject_b);
    survivor_item.snapshot = Some(serde_json::json!({"entity": "survivor-value"}));
    store.create_memory_item(&survivor_item, "tester").unwrap();
    let key_rows_b = subject_key_rows(&store, &subject_b);
    assert_eq!(key_rows_b.len(), 1, "{key_rows_b:?}");

    let request = unique("req");
    store
        .erase_subject(&spec(&bank, &subject_a, &request))
        .unwrap();

    assert_erased(
        &store, &store, &bank, &subject_a, &request, &parent, &item_a,
    );
    // A's key state is visible to a verifier: one shredded row with a stamp.
    let erased_rows = subject_key_rows(&store, &subject_a);
    assert_eq!(erased_rows.len(), 1, "{erased_rows:?}");
    assert_eq!(erased_rows[0].0, "shredded", "{erased_rows:?}");
    assert!(erased_rows[0].2.is_some(), "{erased_rows:?}");
    ErasedBesideSurvivor {
        store,
        bank,
        subject_b,
        item_b,
        key_rows_b,
    }
}

/// Erasing one subject leaves the other subject's items readable and
/// decryptable.
#[test]
fn sqlite_erase_keeps_the_other_subject_items_readable() {
    let ErasedBesideSurvivor {
        store,
        bank,
        item_b,
        ..
    } = erased_beside_a_survivor();
    let survivor = store.get_memory_item(&bank, &item_b).unwrap().unwrap();
    assert_eq!(
        survivor.snapshot,
        Some(serde_json::json!({"entity": "survivor-value"}))
    );
    assert_eq!(
        store.get_gist(&bank, &item_b).unwrap().as_deref(),
        Some("secret gist text")
    );
}

/// Erasing one subject leaves the other subject's key row unchanged and its
/// key alive.
#[test]
fn sqlite_erase_keeps_the_other_subject_key_row_unchanged() {
    let ErasedBesideSurvivor {
        store,
        subject_b,
        key_rows_b,
        ..
    } = erased_beside_a_survivor();
    assert!(!store.subject_dek_destroyed(&subject_b).unwrap());
    assert_eq!(subject_key_rows(&store, &subject_b), key_rows_b);
}

/// A derivative shared by both subjects: erasing A leaves no readable copy of
/// it, while B's originals still read and B's key is untouched.
#[test]
fn sqlite_erase_purges_a_derivative_shared_with_the_other_subject() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("shared-derivative");
    let subject_a = unique("subject-a");
    let subject_b = unique("subject-b");
    let item_a = unique("item-a");
    store
        .create_memory_item(&memory_item(&bank, &item_a, &subject_a), "tester")
        .unwrap();
    let item_b = unique("item-b");
    store
        .create_memory_item(&memory_item(&bank, &item_b, &subject_b), "tester")
        .unwrap();
    let hub_subject = unique("hub-subject");
    let derivative = seed_shared_derivative(&store, &bank, &hub_subject, &item_a, &item_b);
    assert!(
        store.get_snapshot(&bank, &derivative).unwrap().is_some(),
        "the shared derivative quotes its sources before the erase"
    );
    assert!(
        stored_ciphertext(&store, &derivative).is_some(),
        "and its sealed bytes are in the row before the erase"
    );

    let outcome = store
        .erase_subject(&spec(&bank, &subject_a, &unique("req")))
        .unwrap();

    assert_eq!(
        outcome.derived_items_purged, 1,
        "the shared derivative purges"
    );
    assert!(store.get_snapshot(&bank, &derivative).is_err());
    assert!(store.get_gist(&bank, &derivative).is_err());
    assert!(store.get_memory_item(&bank, &derivative).is_err());
    assert_eq!(
        stored_ciphertext(&store, &derivative),
        None,
        "no sealed derivative bytes survive in the row"
    );
    // B's original reads fine, so the purge reached the derivative only.
    assert_eq!(
        store.get_snapshot(&bank, &item_b).unwrap(),
        Some(serde_json::json!({"entity": "secret-value"}))
    );
    assert_eq!(
        store.get_gist(&bank, &item_b).unwrap().as_deref(),
        Some("secret gist text")
    );
    assert!(!store.subject_dek_destroyed(&subject_b).unwrap());
    assert!(
        !store.subject_dek_destroyed(&hub_subject).unwrap(),
        "the hub subject's own key is untouched by the purge"
    );
}

/// A later ordinary write for the erased subject is refused by the store's DEK
/// guard and leaves the key rows exactly as the erasure left them.
#[test]
fn sqlite_erase_refuses_a_later_item_write_without_recreating_the_key() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("refuse-write");
    let subject = unique("subject");
    store
        .create_memory_item(&memory_item(&bank, &unique("item"), &subject), "tester")
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    let shredded = subject_key_rows(&store, &subject);
    assert_eq!(shredded.len(), 1, "{shredded:?}");

    let err = store
        .create_memory_item(
            &memory_item(&bank, &unique("item-late"), &subject),
            "tester",
        )
        .unwrap_err();

    assert_eq!(err.code, ErrorCode::ErasedSubject);
    assert_eq!(
        subject_key_rows(&store, &subject),
        shredded,
        "the refused write adds no key row"
    );
    assert_eq!(
        kms.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject,
        "the key is never recreated"
    );
    assert!(store.subject_dek_destroyed(&subject).unwrap());
}

/// The store's import-facing batch route refuses an item for the erased subject
/// whole: nothing lands and no key row appears.
#[test]
fn sqlite_erase_refuses_an_import_batch_without_recreating_the_key() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("refuse-import");
    let subject = unique("subject");
    store
        .create_memory_item(&memory_item(&bank, &unique("item"), &subject), "tester")
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    let shredded = subject_key_rows(&store, &subject);

    let imported = unique("item-import");
    let batch = [BatchWrite::CreateItem {
        item: memory_item(&bank, &imported, &subject),
        actor: "sync-apply".into(),
    }];
    let err = store.apply_batch(&batch).unwrap_err();

    assert_eq!(err.code, ErrorCode::ErasedSubject);
    assert!(
        store.get_memory_item(&bank, &imported).unwrap().is_none(),
        "the refused batch leaves no row behind"
    );
    assert_eq!(subject_key_rows(&store, &subject), shredded);
    assert!(
        store.subject_is_shredded(&subject).unwrap(),
        "the erasure probe an import layer consults reports shredded"
    );
    assert_eq!(
        kms.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}

/// One bank whose subject's key was destroyed straight through the KMS, plus
/// the outcome of the erasure that followed it and the key rows it left.
struct DirectKeyLoss {
    store: SqliteStore,
    kms: Arc<dyn DekProvider>,
    bank: String,
    subject: String,
    outcome: SubjectEraseOutcome,
    erased_key_rows: Vec<(String, String, Option<String>)>,
}

/// Destroy one subject's key straight through the KMS, then erase the subject:
/// the setup both direct-key-loss claims share. The loss happens outside the
/// store's transaction, so the store's own rows do not move and the two effects
/// are independent; the erasure's own effect is pinned here too.
fn erased_after_a_direct_key_loss() -> DirectKeyLoss {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("pre-destroyed");
    let subject = unique("subject");
    let (item_a, parent) = seed(&store, &bank, &subject);
    let key_rows = subject_key_rows(&store, &subject);
    assert_eq!(key_rows.len(), 1, "{key_rows:?}");
    // The key loss happens outside the store's transaction, so the store's own
    // rows do not move: the two effects are independent.
    kms.destroy_dek(&subject).unwrap();
    assert!(store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(subject_key_rows(&store, &subject), key_rows);

    // The erasure still performs its SQL work, and the key stays destroyed.
    let request = unique("req");
    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap();
    assert_erased(&store, &store, &bank, &subject, &request, &parent, &item_a);
    let erased_key_rows = subject_key_rows(&store, &subject);
    DirectKeyLoss {
        store,
        kms,
        bank,
        subject,
        outcome,
        erased_key_rows,
    }
}

/// A key destroyed straight through the KMS before the erasure stays destroyed
/// while the erasure's own SQL work completes.
#[test]
fn sqlite_erase_after_a_direct_key_loss_still_completes_the_sql_work() {
    let DirectKeyLoss {
        store,
        kms,
        subject,
        outcome,
        erased_key_rows,
        ..
    } = erased_after_a_direct_key_loss();
    assert!(outcome.vectors_purged >= 1, "{outcome:?}");
    assert!(outcome.memtree_nodes_purged >= 2, "{outcome:?}");
    assert_eq!(outcome.tombstones_written, 2);
    assert_eq!(erased_key_rows.len(), 1, "{erased_key_rows:?}");
    assert_eq!(erased_key_rows[0].0, "shredded", "{erased_key_rows:?}");
    assert!(store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(
        kms.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}

/// A retried erasure over a key destroyed straight through the KMS never
/// recreates it.
#[test]
fn sqlite_erase_after_a_direct_key_loss_never_recreates_the_key_on_a_retry() {
    let DirectKeyLoss {
        store,
        kms,
        bank,
        subject,
        erased_key_rows,
        ..
    } = erased_after_a_direct_key_loss();
    store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .expect("the retried erasure completes");
    assert_eq!(
        subject_key_rows(&store, &subject),
        erased_key_rows,
        "the retry adds no key row"
    );
    assert!(store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(
        kms.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}
