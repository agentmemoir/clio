// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres record change feeds and the journal append.
//!
//! ## Owns
//! - `journal_append_pg`, `feed_items_pg`, `feed_assoc_pg`, `feed_triples_pg`,
//!   `feed_persona_stable_pg`, and `feed_persona_prefs_pg`
//!
//! ## Does not own
//! - The persona/belief apply primitives (`postgres_sync_records_tests`)
//!
//! ## Boundary
//!
//! Tests here break when Postgres feed SQL changes; they never change it.

#![allow(clippy::assert_is_empty)]

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::sync_store::SyncStore;
use crate::test_unique::unique;
use crate::{DekProvider, LocalDevKms, PostgresStore};
use std::sync::Arc;

fn open_pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("postgres")
}

#[test]
#[allow(clippy::too_many_lines)]
fn pg_sync_record_feeds_and_journal_cover_every_body() {
    use crate::store::Store;
    use crate::sync_store::SyncJournalRow;

    let _guard = lock_pg_test();
    let s = open_pg();
    let bank = unique("pg-feed");

    let row = SyncJournalRow {
        seq: 0,
        sync_mutation_id: unique("mut"),
        bank_id: bank.clone(),
        entity_kind: "item".into(),
        entity_id: unique("item"),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content_hash: "hash-a".into(),
        content_sealed: None,
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let first = super::journal_append_pg(&s, &row).expect("append");
    assert!(first.inserted);
    assert_eq!(first.seq, 1);
    let again = super::journal_append_pg(&s, &row).expect("replay");
    assert!(!again.inserted);
    assert_eq!(again.seq, 1);

    let item_id = unique("pg-feed-item");
    let item = clio_types::MemoryItem {
        id: item_id.clone(),
        bank_id: bank.clone(),
        subject_id: unique("pg-feed-subject"),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::TaskSpec),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.5),
        source_ref: None,
        snapshot: Some(serde_json::json!({"k": "v"})),
        gist: Some("gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    };
    s.create_memory_item(&item, "agent").expect("item");
    let items = super::feed_items_pg(&s, &bank, "1970-01-01T00:00:00Z", 200).expect("items");
    let found = items
        .iter()
        .find(|r| r.entity_id == item_id)
        .expect("item row");
    assert_eq!(found.entity_kind, "item");
    assert_eq!(found.op, "upsert");
    assert!(
        super::feed_items_pg(&s, &bank, "9999-01-01T00:00:00Z", 200)
            .expect("items")
            .is_empty()
    );

    let edge = crate::model::AssocEdgeRecord {
        id: unique("pg-feed-edge"),
        bank_id: bank.clone(),
        source_id: item_id.clone(),
        target_id: item_id.clone(),
        assoc_kind: crate::model::AssocKind::Explicit,
        relationship: Some("related".into()),
        weight: 0.5,
        last_updated_at: "2026-01-01T00:00:00Z".into(),
    };
    s.put_assoc_edge(&edge).expect("assoc");
    let assoc = super::feed_assoc_pg(&s, &bank, "1970-01-01T00:00:00Z", 200).expect("assoc");
    assert_eq!(assoc.len(), 1);
    assert_eq!(assoc[0].entity_kind, "assoc_edge");
    assert_eq!(assoc[0].entity_id, edge.id);

    let triple_item_id = unique("pg-feed-triple-item");
    let mut carrier = item.clone();
    carrier.id.clone_from(&triple_item_id);
    carrier.subject_id = unique("pg-feed-triple-subject");
    carrier.kind = clio_types::ItemKind::Episodic;
    carrier.category = None;
    carrier.episodic_type = Some(clio_types::EpisodicType::Triple);
    let triple = clio_types::TripleRecord {
        id: unique("pg-feed-triple"),
        bank_id: bank.clone(),
        item_id: triple_item_id.clone(),
        subject: "Alice".into(),
        predicate: "lives_in".into(),
        object: "Boston".into(),
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: None,
    };
    s.commit_triple_add(&carrier, &triple, false, "agent")
        .expect("triple");
    assert!(
        super::feed_triples_pg(&s, &bank, "2026-01-01T00:00:00Z", 200)
            .expect("triples")
            .is_empty()
    );
    s.triple_end(
        &bank,
        "Alice",
        "lives_in",
        Some("Boston"),
        "2026-02-01T00:00:00Z",
        "2026-02-01T00:00:00Z",
    )
    .expect("end");
    let triples = super::feed_triples_pg(&s, &bank, "2026-01-01T00:00:00Z", 200).expect("triples");
    assert_eq!(triples.len(), 1);
    assert_eq!(triples[0].entity_kind, "triple");
    assert_eq!(triples[0].op, "invalidate");

    let stable = clio_types::PersonaStableEntry {
        bank_id: bank.clone(),
        key: "style".into(),
        value: "terse".into(),
        confidence: Some(0.8),
        source: "explicit".into(),
        source_ref: Some("ev".into()),
        context: Some("stable ctx".into()),
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
        valid_until: None,
    };
    s.persona_stable_put(&stable, "agent").expect("stable");
    let stable_feed =
        super::feed_persona_stable_pg(&s, &bank, "1970-01-01T00:00:00Z", 200).expect("stable feed");
    assert_eq!(stable_feed.len(), 1);
    assert_eq!(stable_feed[0].1.value, "terse");
    assert_eq!(stable_feed[0].1.context.as_deref(), Some("stable ctx"));
    assert!(
        super::feed_persona_stable_pg(&s, &bank, "9999-01-01T00:00:00Z", 200)
            .expect("stable feed")
            .is_empty()
    );

    let preference = clio_types::PersonaPreference {
        bank_id: bank.clone(),
        key: "verbosity".into(),
        value: 0.4,
        trend: 0.1,
        confidence: Some(0.7),
        source: "inferred".into(),
        source_ref: Some("ev".into()),
        context: Some("pref ctx".into()),
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
    };
    s.persona_preference_put(&preference, "agent")
        .expect("pref");
    let pref_feed =
        super::feed_persona_prefs_pg(&s, &bank, "1970-01-01T00:00:00Z", 200).expect("pref feed");
    assert_eq!(pref_feed.len(), 1);
    assert!((pref_feed[0].1.value - 0.4).abs() < 1e-12);
    assert_eq!(pref_feed[0].1.context.as_deref(), Some("pref ctx"));

    let empty_bank = unique("pg-feed-empty");
    assert!(
        super::feed_items_pg(&s, &empty_bank, "1970-01-01T00:00:00Z", 200)
            .expect("items")
            .is_empty()
    );
    assert!(
        super::feed_assoc_pg(&s, &empty_bank, "1970-01-01T00:00:00Z", 200)
            .expect("assoc")
            .is_empty()
    );
    assert!(
        super::feed_triples_pg(&s, &empty_bank, "1970-01-01T00:00:00Z", 200)
            .expect("triples")
            .is_empty()
    );
    assert!(
        super::feed_persona_stable_pg(&s, &empty_bank, "1970-01-01T00:00:00Z", 200)
            .expect("stable")
            .is_empty()
    );
    assert!(
        super::feed_persona_prefs_pg(&s, &empty_bank, "1970-01-01T00:00:00Z", 200)
            .expect("prefs")
            .is_empty()
    );
}
