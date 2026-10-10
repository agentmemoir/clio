// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one behavioral parity suite that both store
//! backends must pass: the same contract assertions run against a SQLite
//! store and a Postgres store.
//!
//! ## Owns
//! - `run_parity_suite`, the shared assertions over the `Store` contract
//!
//! ## Does not own
//! - Backend-specific scenarios, which live in each backend's `*_tests` file
//! - The store behaviour the suite exercises
//!
//! ## Boundary
//!
//! Changes to the shared backend contract should normally require changes
//! here. Changes to one backend alone should not require changes here.
//!
//! Keep this module focused on the shared parity suite.

use crate::audit_model::CorrectionSpec;
use crate::dek::{DekProvider, LocalDevKms};
use crate::distill_intent::DistillIntent;
use crate::model::{AssocEdgeRecord, AssocKind, EMBEDDING_DIMS, VectorSlot};
use crate::store::Store;
use clio_types::{EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use std::sync::Arc;

use crate::test_unique::unique;

/// A typed memory-item fixture for the parity suite.
fn memory_item(id: &str, bank: &str, subject: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.0),
        source_ref: Some("turn_9".into()),
        context: None,
        snapshot: Some(serde_json::json!({"bank": bank})),
        gist: Some(gist.into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
    }
}

/// Run contract assertions against any [`Store`] + KMS pair.
pub fn run_parity_suite(store: &dyn Store, kms: &dyn DekProvider) {
    let bank_a = unique("bank-a");
    let bank_b = unique("bank-b");
    let item_1 = unique("item-1");
    let item_2 = unique("item-2");
    let subject = unique("subj");
    let edge_id = unique("edge");

    let item = memory_item(&item_1, &bank_a, &subject, "plaintext-payload");
    store.create_memory_item(&item, "actor").expect("create");
    let got = store
        .get_memory_item(&bank_a, &item_1)
        .expect("get")
        .expect("present");
    assert_eq!(got.gist.as_deref(), Some("plaintext-payload"));
    assert!(store.get_memory_item(&bank_b, &item_1).unwrap().is_none());

    let raw = store
        .inspect_content_ciphertext(&bank_a, &item_1)
        .unwrap()
        .expect("cipher");
    assert!(!raw.contains("plaintext-payload"));
    assert!(raw.contains("ciphertext_b64"));

    let edge = AssocEdgeRecord {
        id: edge_id.clone(),
        bank_id: bank_a.clone(),
        source_id: item_1.clone(),
        target_id: item_2.clone(),
        assoc_kind: AssocKind::Coactivation,
        relationship: None,
        weight: 0.5,
        last_updated_at: "2026-01-01T00:00:00Z".into(),
    };
    store.put_assoc_edge(&edge).unwrap();
    let eg = store.get_assoc_edge(&bank_a, &edge_id).unwrap().unwrap();
    assert!((eg.weight - 0.5).abs() < f64::EPSILON);
    assert!(store.get_assoc_edge(&bank_b, &edge_id).unwrap().is_none());
    assert!(store.delete_assoc_edge(&bank_a, &edge_id).unwrap());

    store
        .create_memory_item(&memory_item(&item_2, &bank_a, &subject, "other"), "actor")
        .unwrap();

    let mut emb_a = vec![0.0_f32; EMBEDDING_DIMS];
    emb_a[0] = 1.0;
    let mut emb_b = vec![0.0_f32; EMBEDDING_DIMS];
    emb_b[0] = 0.9;
    emb_b[1] = 0.1;
    store
        .upsert_vector(&VectorSlot {
            item_id: item_1.clone(),
            bank_id: bank_a.clone(),
            model_id: "BAAI/bge-small-en-v1.5".into(),
            embedding: emb_a.clone(),
        })
        .unwrap();
    store
        .upsert_vector(&VectorSlot {
            item_id: item_2.clone(),
            bank_id: bank_a.clone(),
            model_id: "BAAI/bge-small-en-v1.5".into(),
            embedding: emb_b,
        })
        .unwrap();
    let hits = store.knn(&bank_a, &emb_a, 2).unwrap();
    assert!(!hits.is_empty(), "{hits:?}");
    assert_eq!(hits[0].item_id, item_1);
    assert!(store.delete_vector(&bank_a, &item_2).unwrap());
    let after_delete = store.knn(&bank_a, &emb_a, 0).unwrap();
    assert!(after_delete.is_empty(), "{after_delete:?}");
    assert!(store.knn(&bank_a, &emb_a[..10], 1).is_err());

    let env = kms.encrypt(&subject, b"x").unwrap();
    kms.destroy_dek(&subject).unwrap();
    assert_eq!(
        kms.decrypt(&env).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    store
        .upsert_subject_key_ref(&subject, "local:x:v1", "shredded")
        .unwrap();
    let diag = kms.diagnose_masked(&subject);
    assert!(diag.contains("shredded"));

    let err = store.get_memory_item(&bank_a, &item_1).unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject);
    assert!(store.delete_item(&bank_a, &item_1).unwrap());
    assert!(!store.delete_item(&bank_a, &item_1).unwrap());

    // The KNN primitive carries the same current-belief visibility rule as the
    // index legs: a corrected prior keeps its vector but never returns.
    knn_visibility_parity(store);

    // The claim row is part of the same contract on both backends: the same
    // write, the same read, the same refusals, and the same bank scope.
    distill_intent_parity(store, &bank_a, &bank_b);
}

/// A fixed claim stamp in the millisecond-epoch text a derivation record's
/// `distilled_at` uses. A literal, so nothing here depends on when the suite
/// ran.
const CLAIMED_AT: &str = "1780272000000";

/// Run the distill-intent claim, completion, and read contract against any
/// [`Store`]: the round trip, the refusal of a second claim and of a second
/// completion, an unknown hub, and bank scope.
fn distill_intent_parity(store: &dyn Store, bank_a: &str, bank_b: &str) {
    let hub = unique("intent-hub");
    let first = DistillIntent::new(
        bank_a,
        &hub,
        format!("hub-distill:{bank_a}:{hub}"),
        "tester",
        CLAIMED_AT,
    );
    assert!(store.write_distill_intent(bank_a, &first).unwrap());

    let read = store.get_distill_intent(bank_a, &hub).unwrap().unwrap();
    assert_eq!(
        read, first,
        "the read returns every field it was written with"
    );
    assert!(!read.is_complete(), "a fresh claim is incomplete");
    assert!(
        store.get_distill_intent(bank_b, &hub).unwrap().is_none(),
        "another bank must not resolve the claim"
    );
    assert!(
        store
            .get_distill_intent(bank_a, &unique("intent-absent"))
            .unwrap()
            .is_none(),
        "an unclaimed hub reads back as none"
    );

    // A second claim for the same pair is refused and leaves the first row
    // byte-identical: the first actor, unit id, and claim time are the record.
    let second = DistillIntent::new(
        bank_a,
        &hub,
        unique("intent-unit"),
        "another-actor",
        "1783000000000",
    );
    assert!(!store.write_distill_intent(bank_a, &second).unwrap());
    assert_eq!(
        store.get_distill_intent(bank_a, &hub).unwrap().unwrap(),
        first
    );

    // Bank scope is a write boundary too, not only a read filter: another
    // bank claims the same hub id under its own key, and neither row
    // disturbs the other.
    let other = DistillIntent::new(
        bank_b,
        &hub,
        unique("intent-unit"),
        "other-actor",
        "1783100000000",
    );
    assert!(
        store.write_distill_intent(bank_b, &other).unwrap(),
        "the same hub id in another bank is a different claim"
    );
    assert_eq!(
        store.get_distill_intent(bank_a, &hub).unwrap().unwrap(),
        first,
        "the first bank's row is byte-identical after the other bank's claim"
    );
    assert_eq!(
        store.get_distill_intent(bank_b, &hub).unwrap().unwrap(),
        other,
        "the other bank's row reads back its own values"
    );

    let revision = unique("intent-revision");
    assert!(
        store
            .complete_distill_intent(bank_a, &hub, &revision)
            .unwrap()
    );
    assert!(
        !store
            .complete_distill_intent(bank_a, &hub, &unique("intent-retry"))
            .unwrap(),
        "a completion naming a different revision must be refused"
    );
    let completed = store.get_distill_intent(bank_a, &hub).unwrap().unwrap();
    assert!(completed.is_complete());
    assert_eq!(
        completed.revision_item_id, revision,
        "the first revision stands: a claim is never downgraded"
    );
}

/// KNN visibility parity: a corrected prior keeps its vector row but is never
/// returned, and the live revision is reachable even when the prior ranks
/// nearer. The primitive carries the same rule as the index legs.
fn knn_visibility_parity(store: &dyn Store) {
    let bank = unique("bank-knn");
    let subject = unique("subj");
    let prior_id = unique("knn-prior");
    let live_id = unique("knn-live");

    let prior = memory_item(&prior_id, &bank, &subject, "old belief");
    store.create_memory_item(&prior, "actor").expect("prior");
    let mut corrected = prior.clone();
    corrected.id = live_id.clone();
    corrected.gist = Some("current belief".into());
    corrected.tx_from = "t1".into();
    corrected.valid_from = "t1".into();
    corrected.created_at = "t1".into();
    corrected.updated_at = "t1".into();
    store
        .correct_item(&CorrectionSpec {
            bank_id: bank.clone(),
            prior_item_id: prior_id.clone(),
            new_item: corrected,
            reason: "belief changed".into(),
            actor: "actor".into(),
            timestamp: "t1".into(),
        })
        .expect("correct");

    let mut emb_prior = vec![0.0_f32; EMBEDDING_DIMS];
    emb_prior[0] = 1.0;
    let mut emb_live = vec![0.0_f32; EMBEDDING_DIMS];
    emb_live[1] = 1.0;
    store
        .upsert_vector(&VectorSlot {
            item_id: prior_id,
            bank_id: bank.clone(),
            model_id: "BAAI/bge-small-en-v1.5".into(),
            embedding: emb_prior.clone(),
        })
        .unwrap();
    store
        .upsert_vector(&VectorSlot {
            item_id: live_id.clone(),
            bank_id: bank.clone(),
            model_id: "BAAI/bge-small-en-v1.5".into(),
            embedding: emb_live,
        })
        .unwrap();

    let hits = store.knn(&bank, &emb_prior, 1).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].item_id, live_id, "{hits:?}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStore;

    #[test]
    fn sqlite_parity() {
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let store = SqliteStore::open_in_memory(kms.clone()).expect("sqlite");
        run_parity_suite(&store, kms.as_ref());
    }

    #[test]
    fn sqlite_rejects_bad_inputs() {
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let store = SqliteStore::open_in_memory(kms).unwrap();
        let mut bad = memory_item("", "b", "s", "gist");
        bad.snapshot = Some(serde_json::json!({}));
        assert!(store.create_memory_item(&bad, "actor").is_err());
        let err = store.put_assoc_edge(&AssocEdgeRecord {
            id: "e".into(),
            bank_id: "b".into(),
            source_id: "a".into(),
            target_id: "c".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("rel".into()),
            weight: 2.0,
            last_updated_at: "t".into(),
        });
        assert_eq!(err.unwrap_err().code, ErrorCode::OutOfRange);
        assert!(
            store
                .put_assoc_edge(&AssocEdgeRecord {
                    id: String::new(),
                    bank_id: "b".into(),
                    source_id: "a".into(),
                    target_id: "c".into(),
                    assoc_kind: AssocKind::Explicit,
                    relationship: None,
                    weight: 0.1,
                    last_updated_at: "t".into(),
                })
                .is_err()
        );
    }

    #[test]
    fn postgres_parity() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        assert!(
            url.starts_with("postgres"),
            "CLIO_DATABASE_URL must be a postgres URL for store parity"
        );
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let store = crate::postgres::PostgresStore::open(&url, kms.clone())
            .unwrap_or_else(|e| panic!("postgres+pgvector required (compose up): {e}"));
        run_parity_suite(&store, kms.as_ref());
    }
}
