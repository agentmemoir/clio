// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres parity tests of the export-read
//! primitives: `list_triples`, `list_beliefs`, and `list_tombstones`.
//!
//! ## Owns
//! - Postgres contract assertions for the three bank-wide read listings
//!
//! ## Does not own
//! - SQLite listing behavior (see `triple_tests` / `belief_tests` / `erase_tests`)
//! - Export/import orchestration (`clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to Postgres listing SQL should normally update tests here.
//! Changes to the export/import orchestration should not require changes here.
//!
//! Keep this module focused on Postgres export-read parity.

use crate::belief_store::BeliefStore;
use crate::compliance_store::{ComplianceStore, SubjectEraseSpec};
use crate::db_url::postgres_url_or_default;
use crate::dek::{DekProvider, LocalDevKms};
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::store::Store;
use clio_types::{BeliefObject, ConfidenceEntry, SourceType};
use std::sync::Arc;

use crate::test_unique::unique;

fn open_pg() -> Option<PostgresStore> {
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = match PostgresStore::open(&url, kms) {
        Ok(store) => store,
        Err(err) => {
            eprintln!(
                "{}",
                crate::pg_skip::pg_skip_note("postgres export-read tests", &err)
            );
            return None;
        }
    };
    Some(store)
}

#[test]
fn postgres_list_triples_returns_open_and_closed_rows() {
    let _guard = lock_pg_test();
    let Some(store) = open_pg() else {
        return;
    };
    let bank = unique("pg-list-triples");
    let carrier = |id: &str, tx: &str| clio_types::MemoryItem {
        id: id.into(),
        bank_id: bank.clone(),
        subject_id: "subj".into(),
        kind: clio_types::ItemKind::Episodic,
        category: None,
        episodic_type: Some(clio_types::EpisodicType::Triple),
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.85),
        source_ref: None,
        snapshot: None,
        gist: Some("pg listing probe".into()),
        valid_from: tx.into(),
        valid_until: None,
        tx_from: tx.into(),
        tx_until: None,
        created_at: tx.into(),
        updated_at: tx.into(),
        context: None,
    };
    let edge = |id: &str, item_id: &str, object: &str, tx: &str| clio_types::TripleRecord {
        context: None,
        id: id.into(),
        bank_id: bank.clone(),
        item_id: item_id.into(),
        subject: "Alice".into(),
        predicate: "lives_in".into(),
        object: object.into(),
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        valid_from: tx.into(),
        valid_until: None,
        tx_from: tx.into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: tx.into(),
        updated_at: None,
    };
    let older = unique("older");
    let newer = unique("newer");
    let c1 = carrier(&older, "2026-01-01T00:00:00Z");
    let e1 = edge(&older, &older, "Boston", "2026-01-01T00:00:00Z");
    store.commit_triple_add(&c1, &e1, false, "tester").unwrap();
    let c2 = carrier(&newer, "2026-02-01T00:00:00Z");
    let e2 = edge(&newer, &newer, "NYC", "2026-02-01T00:00:00Z");
    store.commit_triple_add(&c2, &e2, true, "tester").unwrap();
    let listed = store.list_triples(&bank).unwrap();
    let ids: Vec<&str> = listed.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, vec![older.as_str(), newer.as_str()], "oldest tx first");
    assert!(listed[0].tx_until.is_some(), "superseded edge is closed");
    assert!(listed[1].tx_until.is_none(), "current edge stays open");
    let other = store.list_triples(&unique("pg-empty")).unwrap();
    assert!(other.is_empty(), "{other:?}");
}

#[test]
fn postgres_list_beliefs_returns_full_trajectories() {
    let _guard = lock_pg_test();
    let Some(store) = open_pg() else {
        return;
    };
    let bank = unique("pg-list-beliefs");
    let id = unique("bel");
    let belief = BeliefObject {
        context: None,
        context_as_of: None,
        id: id.clone(),
        bank_id: bank.clone(),
        subject_id: "subj-pg".into(),
        proposition: "pg listing belief probe".into(),
        confidence_history: vec![
            ConfidenceEntry::new(
                "2026-01-01T00:00:00Z",
                0.4,
                "turn_1",
                SourceType::UserStated,
            )
            .unwrap(),
        ],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    store.create_belief(&belief, "tester").unwrap();
    store
        .append_confidence(
            &bank,
            &id,
            &ConfidenceEntry::new(
                "2026-02-01T00:00:00Z",
                0.8,
                "turn_2",
                SourceType::AgentInferred,
            )
            .unwrap(),
            "tester",
        )
        .unwrap();
    let listed = store.list_beliefs(&bank).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].confidence_history.len(), 2);
    let empty_bank_beliefs = store.list_beliefs(&unique("pg-empty")).unwrap();
    assert!(empty_bank_beliefs.is_empty(), "{empty_bank_beliefs:?}");
}

#[test]
fn postgres_list_tombstones_returns_rows_for_the_bank() {
    let _guard = lock_pg_test();
    let Some(store) = open_pg() else {
        return;
    };
    let bank = unique("pg-list-tombs");
    // The erase below crypto-shreds this subject; a fixed id would poison every
    // later run against the shared database, so it must be unique like the bank.
    let subject = unique("subj-tomb");
    let carrier = clio_types::MemoryItem {
        id: unique("gone"),
        bank_id: bank.clone(),
        subject_id: subject.clone(),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({"k": "v"})),
        gist: None,
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    };
    store.create_memory_item(&carrier, "tester").unwrap();
    let request = unique("req");
    // A fresh salt per run: no test asserts the tombstone hash value.
    let salt = rand::random::<[u8; 16]>();
    store
        .erase_subject(&SubjectEraseSpec {
            bank_id: &bank,
            subject_id: &subject,
            request_id: &request,
            legal_basis: "GDPR_ART_17",
            reason_code: "COMPLIANCE_CRYPTO_SHRED",
            deleted_at: "2026-06-01T00:00:00Z",
            salt: &salt,
        })
        .unwrap();
    // A subject with a key but no items writes a subject-level (null hash) row.
    let key_only = unique("key-only");
    store
        .upsert_subject_key_ref(&key_only, "local:key-only:v1", "active")
        .unwrap();
    let key_request = unique("req");
    store
        .erase_subject(&SubjectEraseSpec {
            bank_id: &bank,
            subject_id: &key_only,
            request_id: &key_request,
            legal_basis: "GDPR_ART_17",
            reason_code: "COMPLIANCE_CRYPTO_SHRED",
            deleted_at: "2026-06-01T00:00:00Z",
            salt: &salt,
        })
        .unwrap();

    let tombs = store.list_tombstones(&bank).unwrap();
    assert!(!tombs.is_empty(), "{tombs:?}");
    assert!(tombs.iter().all(|t| t.legal_basis == "GDPR_ART_17"));
    let hashes: Vec<Option<&str>> = tombs.iter().map(|t| t.item_id_hash.as_deref()).collect();
    let first_null = hashes
        .iter()
        .position(|h| h.is_none())
        .expect("key-only tombstone has no item hash");
    assert!(hashes[..first_null].iter().all(Option::is_some));
    assert!(hashes[first_null..].iter().all(Option::is_none));
    let empty_bank_tombs = store.list_tombstones(&unique("pg-empty")).unwrap();
    assert!(empty_bank_tombs.is_empty(), "{empty_bank_tombs:?}");
}
