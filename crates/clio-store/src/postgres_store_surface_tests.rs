// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! Calls every method `postgres_store` exposes in its `Store` and
//! `TripleAssertion` impls, through the traits, on a real Postgres store.
//!
//! ## Owns
//! - One call and one assertion per method, grouped by cluster
//!
//! ## Does not own
//! - The MemTree gist envelope (`postgres_store_tests`)
//! - The SQL bodies the methods delegate to (the backend modules)
//!
//! ## Boundary
//!
//! Test-only. A change to which methods `postgres_store` exposes belongs here.

use crate::HygieneStore;
use crate::audit_model::CorrectionSpec;
use crate::dek::{DekProvider, LocalDevKms};
use crate::distilled_unit::DistilledUnitRecord;
use crate::distilled_unit::distilled_unit_tests::hub_item;
use crate::model::{AssocEdgeRecord, AssocKind, ItemInspectFilter, VectorSlot};
use crate::postgres::PostgresStore;
use crate::store::Store;
use crate::test_unique::unique;
use crate::triple_assertion::TripleAssertion;
use clio_types::{
    EpisodicType, EpistemicKind, ItemKind, MemoryItem, SemanticCategory, TimeAxis, TripleQuery,
    TripleRecord,
};
use serde_json::json;
use std::sync::Arc;

/// The shared test database, opened the way the sibling Postgres suites open it.
fn pg() -> PostgresStore {
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("the test database opens")
}

/// A seedable semantic fact whose stamps are canonical.
fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("{bank}-subject"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.7),
        admission_score: Some(0.5),
        source_ref: Some("turn-1".into()),
        snapshot: Some(json!({"k": id})),
        gist: Some("surface gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// One explicit edge between two ids inside `bank`.
fn edge(id: &str, bank: &str) -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: id.into(),
        bank_id: bank.into(),
        source_id: format!("{bank}-from"),
        target_id: format!("{bank}-to"),
        assoc_kind: AssocKind::Explicit,
        relationship: Some("related_to".into()),
        weight: 0.5,
        last_updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

/// The episodic carrier a triple add writes beside the edge.
fn carrier(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Triple),
        ..item(id, bank)
    }
}

/// The triple the carrier above asserts.
fn triple(id: &str, bank: &str, item_id: &str) -> TripleRecord {
    TripleRecord {
        id: id.into(),
        bank_id: bank.into(),
        item_id: item_id.into(),
        subject: "Alice".into(),
        predicate: "lives_in".into(),
        object: "Lisbon".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: None,
    }
}

#[test]
fn envelope_crud_and_subject_lookup_round_trip() {
    let _guard = crate::lock_pg_test();
    let store = pg();
    let bank = unique("surface-envelope");
    let id = unique("surface-item");
    let seeded = item(&id, &bank);
    store.create_memory_item(&seeded, "agent").unwrap();

    let read = store.get_memory_item(&bank, &id).unwrap().unwrap();
    assert_eq!(read.gist, seeded.gist);
    assert_eq!(store.get_snapshot(&bank, &id).unwrap(), seeded.snapshot);
    let gist = store.get_gist(&bank, &id).unwrap();
    assert_eq!(gist.as_deref(), Some("surface gist"));
    let other = unique("surface-other-bank");
    assert!(store.get_memory_item(&other, &id).unwrap().is_none());

    let mut revised = seeded.clone();
    revised.gist = Some("surface gist, revised".into());
    store.update_memory_item(&revised, "agent").unwrap();
    let gist = store.get_gist(&bank, &id).unwrap();
    assert_eq!(gist.as_deref(), Some("surface gist, revised"));

    let subject = seeded.subject_id.clone();
    let by_subject = store.get_item_by_subject(&bank, &subject, None).unwrap();
    assert_eq!(by_subject.unwrap().id, id);
    let as_of = store.get_item_by_subject(&bank, &subject, Some(&seeded.tx_from));
    assert!(as_of.unwrap().is_some(), "the open row is live at tx_from");
    let cross = store.get_item_by_subject(&other, &subject, None).unwrap();
    assert!(cross.is_none(), "the bank guards the lookup");

    assert!(store.delete_item(&bank, &id).unwrap());
    assert!(store.get_memory_item(&bank, &id).unwrap().is_none());
    assert!(store.get_snapshot(&bank, &id).unwrap().is_none());
    assert!(store.get_gist(&bank, &id).unwrap().is_none());
    assert!(
        !store.delete_item(&bank, &id).unwrap(),
        "a gone row is false"
    );
}

#[test]
fn association_edges_round_trip_reinforce_and_audit() {
    let _guard = crate::lock_pg_test();
    let store = pg();
    let bank = unique("surface-assoc");
    let created = edge(&unique("surface-edge"), &bank);
    store.put_assoc_edge(&created).unwrap();

    let got = store.get_assoc_edge(&bank, &created.id).unwrap().unwrap();
    assert_eq!(got.weight, created.weight);
    assert_eq!(got.relationship, created.relationship);
    let other = unique("surface-other-bank");
    assert!(store.get_assoc_edge(&other, &created.id).unwrap().is_none());

    let for_item = store.list_assoc_edges(&bank, Some(&created.source_id));
    let for_item = for_item.unwrap();
    assert_eq!(for_item.len(), 1, "{for_item:?}");
    assert_eq!(for_item[0].id, created.id);
    assert_eq!(store.list_assoc_edges(&bank, None).unwrap().len(), 1);

    let (from, to) = (created.source_id.clone(), created.target_id.clone());
    let reinforced = store.reinforce_assoc_edge(&bank, &from, &to, 0, 0.5);
    let reinforced = reinforced.unwrap();
    assert_eq!(reinforced.bank_id, bank);
    assert!((0.0..=1.0).contains(&reinforced.weight), "{reinforced:?}");
    let audit = json!({"edge_id": created.id});
    store
        .record_assoc_audit(&bank, "graph_link", Some(&from), "agent", &audit)
        .unwrap();

    assert!(store.delete_assoc_edge(&bank, &created.id).unwrap());
    assert!(store.get_assoc_edge(&bank, &created.id).unwrap().is_none());
    let again = store.delete_assoc_edge(&bank, &created.id);
    assert!(!again.unwrap(), "a missing edge reports false");
}

#[test]
fn triples_commit_read_query_list_end_and_invalidate() {
    let _guard = crate::lock_pg_test();
    let store = pg();
    let bank = unique("surface-triple");
    let carrier_id = unique("surface-carrier");
    let edge_id = unique("surface-triple-edge");
    let c = carrier(&carrier_id, &bank);
    let t = triple(&edge_id, &bank, &carrier_id);
    store.commit_triple_add(&c, &t, true, "agent").unwrap();

    let got = store.get_triple(&bank, &edge_id).unwrap().unwrap();
    assert_eq!(got.object, "Lisbon");
    assert_eq!(got.subject, "Alice");
    let other = unique("surface-other-bank");
    assert!(store.get_triple(&other, &edge_id).unwrap().is_none());

    let query = TripleQuery {
        bank_id: bank.clone(),
        subject: Some("Alice".into()),
        predicate: Some("lives_in".into()),
        object: None,
        as_of: None,
        time_axis: TimeAxis::Valid,
    };
    let triples = store.query_triples(&query).unwrap();
    let queried: Vec<String> = triples.into_iter().map(|t| t.id).collect();
    assert_eq!(queried, vec![edge_id.clone()]);
    assert_eq!(store.list_triples(&bank).unwrap().len(), 1);
    assert_eq!(store.list_triples(&other).unwrap().len(), 0);

    let ever = store.triple_asserted_ever(&bank, "Alice", "lives_in", Some("Lisbon"));
    assert!(ever.unwrap(), "the object was asserted");
    let miss = store.triple_asserted_ever(&bank, "Alice", "lives_in", Some("Paris"));
    assert!(!miss.unwrap(), "a different object was never asserted");

    let end = store.triple_end(&bank, "Alice", "lives_in", None, "2026-02-01", "2026-02-01");
    let ended = end.unwrap();
    assert_ne!(ended.closed_ids.len(), 0, "{ended:?}");
    let after = store.triple_asserted_ever(&bank, "Alice", "lives_in", None);
    assert!(after.unwrap(), "an ended triple was still asserted once");

    let live_id = unique("surface-invalidate");
    let live = item(&live_id, &bank);
    store.create_memory_item(&live, "agent").unwrap();
    let close = store.invalidate_item(&bank, &live_id, "2026-02-01", "2026-02-01", None, "agent");
    assert_eq!(close.unwrap().closed_ids, vec![live_id.clone()]);
    let closed = store.get_memory_item(&bank, &live_id).unwrap().unwrap();
    assert_eq!(closed.tx_until.as_deref(), Some("2026-02-01"));
}

#[test]
fn lifecycle_identity_inspect_window_archive_and_discard() {
    let _guard = crate::lock_pg_test();
    let store = pg();
    let bank = unique("surface-life");
    let id = unique("surface-life-item");
    let seeded = item(&id, &bank);
    store.create_memory_item(&seeded, "agent").unwrap();

    let identity = store.get_item_identity(&bank, &id).unwrap().unwrap();
    assert_eq!(identity.item_id, id);
    assert_eq!(identity.epistemic_kind, "fact");
    let missing = unique("surface-missing");
    assert!(store.get_item_identity(&bank, &missing).unwrap().is_none());

    let filter = ItemInspectFilter::new(&bank);
    let rows = store.inspect_items(&filter).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].item_id, id);
    assert_eq!(store.count_items(&filter).unwrap(), 1);

    let (start, end) = ("2025-12-31T00:00:00Z", "2026-12-31T00:00:00Z");
    let window = store.item_ids_in_window(&bank, TimeAxis::Valid, start, end, 10);
    assert_eq!(window.unwrap(), vec![id.clone()]);

    let archived_at = "2026-01-02T00:00:00Z";
    store
        .archive_item(&bank, &id, archived_at, "agent")
        .unwrap();
    assert!(store.item_is_archived(&bank, &id).unwrap());
    assert!(!store.item_is_archived(&bank, &missing).unwrap());

    let discarded = store.discard_item(&bank, &id, "ops cleanup", "2026-01-03", "agent");
    assert_eq!(discarded.unwrap().item_id, id);
    assert!(store.get_memory_item(&bank, &id).unwrap().is_some());
}

#[test]
fn correction_records_audit_trails_over_both_revisions() {
    let _guard = crate::lock_pg_test();
    let store = pg();
    let bank = unique("surface-correct");
    let prior_id = unique("surface-prior");
    let next_id = unique("surface-next");
    let prior = item(&prior_id, &bank);
    store.create_memory_item(&prior, "agent").unwrap();
    let note = json!({"note": "ok"});
    store
        .record_audit_event(&bank, "validate_attest", Some(&prior_id), "agent", &note)
        .unwrap();

    let mut next = prior.clone();
    next.id = next_id.clone();
    next.gist = Some("surface gist, corrected".into());
    let spec = CorrectionSpec {
        bank_id: bank.clone(),
        prior_item_id: prior_id.clone(),
        new_item: next,
        reason: "the meeting moved".into(),
        actor: "user".into(),
        timestamp: "2026-01-05T00:00:00Z".into(),
    };
    let outcome = store.correct_item(&spec).unwrap();
    assert_eq!(outcome.prior_item_id, prior_id);
    assert_eq!(outcome.new_item_id, next_id);
    assert!(!outcome.already_superseded);

    let trail = store.audit_trail(&bank, &prior_id).unwrap();
    assert!(
        trail
            .events
            .iter()
            .any(|e| e.operation == "validate_attest")
    );
    let history = store.audit_trail(&bank, &next_id).unwrap();
    assert!(
        history.events.iter().any(|e| e.operation == "correct"),
        "{history:?}"
    );
}

#[test]
fn distilled_units_commit_and_read_back_bank_scoped() {
    let _guard = crate::lock_pg_test();
    let store = pg();
    let bank = unique("surface-distilled");
    let hub = unique("surface-hub");
    let source = unique("surface-source");
    let subject = unique("surface-subject");
    let item_id = format!("hub-distill:{bank}:{hub}");
    let seeded = hub_item(&bank, &item_id, &subject, &hub, &[&source]);
    let record = DistilledUnitRecord::new(
        &bank,
        &item_id,
        &hub,
        vec![source.clone()],
        "hub_distilled",
        "2026-06-01T00:00:00Z",
    );
    store
        .commit_distilled_unit(&seeded, &record, "tester")
        .unwrap();

    let read = store.get_distilled_unit(&bank, &item_id).unwrap().unwrap();
    assert_eq!(read, record);
    let missing = unique("surface-missing");
    assert!(store.get_distilled_unit(&bank, &missing).unwrap().is_none());
    let other = unique("surface-other-bank");
    assert!(
        store
            .get_distilled_unit(&other, &item_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn vectors_upsert_delete_knn_and_report_the_configured_space() {
    let _guard = crate::lock_pg_test();
    let store = pg();
    let bank = unique("surface-vector");
    let id = unique("surface-vector-item");
    let seeded = item(&id, &bank);
    store.create_memory_item(&seeded, "agent").unwrap();
    let space = store.embedding_space().unwrap();
    assert_eq!(&space, store.space(), "the trait reports the space");
    assert!(space.dims > 0 && !space.model_id.is_empty(), "{space:?}");

    let slot = VectorSlot {
        item_id: id.clone(),
        bank_id: bank.clone(),
        model_id: space.model_id.clone(),
        embedding: vec![0.0_f32; space.dims],
    };
    store.upsert_vector(&slot).unwrap();
    store.upsert_vector(&slot).unwrap();

    let hits = store.knn(&bank, &slot.embedding, 5).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].item_id, id);
    assert_eq!(store.knn(&bank, &slot.embedding, 0).unwrap().len(), 0);

    assert!(store.delete_vector(&bank, &id).unwrap());
    assert!(!store.delete_vector(&bank, &id).unwrap(), "a gone slot");
}

#[test]
fn subject_key_refs_and_ciphertext_inspection() {
    let _guard = crate::lock_pg_test();
    let store = pg();
    let subject = unique("surface-key");
    let dek_ref = format!("local:{subject}:v1");
    store
        .upsert_subject_key_ref(&subject, &dek_ref, "active")
        .unwrap();
    store
        .upsert_subject_key_ref(&subject, &dek_ref, "shredded")
        .unwrap();

    let bank = unique("surface-cipher");
    let id = unique("surface-cipher-item");
    let seeded = item(&id, &bank);
    store.create_memory_item(&seeded, "agent").unwrap();

    let stored = store
        .inspect_content_ciphertext(&bank, &id)
        .unwrap()
        .unwrap();
    assert!(!stored.is_empty(), "the envelope carries a body");
    assert!(!stored.contains("surface gist"), "no plaintext: {stored}");
    let missing = unique("surface-missing");
    assert!(
        store
            .inspect_content_ciphertext(&bank, &missing)
            .unwrap()
            .is_none()
    );
    let other = unique("surface-other-bank");
    assert!(
        store
            .inspect_content_ciphertext(&other, &id)
            .unwrap()
            .is_none()
    );
}
