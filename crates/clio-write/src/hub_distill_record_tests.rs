// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the distilled-unit derivation-record tests:
//! the record is written with the item, keyed by the item's own id, agrees with
//! the written snapshot, survives a reopen, is not duplicated by a second wave,
//! and a storage failure leaves no untracked unit.
//!
//! ## Owns
//! - Record-vs-snapshot agreement and the id-keyed lookups around distillation
//!
//! ## Does not own
//! - The record's storage shape, SQL, and atomicity (`clio-store`) or the shared
//!   engine fixtures (`hub_distill_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::hub_distill_tests::{ctx, engine, seed_hub, semantic, store};
use super::{HUB_DISTILL_JOB, HUB_DISTILL_MARKER};
use clio_config::retention::ResolvedRetention;
use clio_store::{DISTILLED_UNIT_RECORD_VERSION, LocalDevKms, SqliteStore, Store};
use clio_types::ErrorCode;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The distilled item id a hub yields in these tests.
const DISTILLED: &str = "hub-distill:bank:hub";

/// A scratch sqlite path no other call in this process produces.
///
/// Same shape as `clio-store`'s `test_unique`: the counter separates calls in
/// one binary, the process id separates concurrent test binaries, and the clock
/// keeps a re-run from reusing a path a previous run left behind.
fn scratch_db(prefix: &str) -> std::path::PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!(
        "{prefix}-{nanos}-{}-{sequence}.db",
        std::process::id()
    ))
}

/// The claim set implied by a written summary snapshot (hub id + neighbours).
fn snapshot_claim_ids(store: &dyn Store, bank: &str, item_id: &str) -> BTreeSet<String> {
    let snapshot = store
        .get_snapshot(bank, item_id)
        .expect("snapshot read")
        .expect("distilled item must keep its snapshot");
    let mut ids = BTreeSet::new();
    if let Some(hub) = snapshot.get("hub_id").and_then(serde_json::Value::as_str) {
        ids.insert(hub.to_owned());
    }
    for id in snapshot
        .get("neighbor_ids")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
    {
        ids.insert(id.to_owned());
    }
    ids
}

#[test]
fn the_record_is_keyed_by_the_item_id_and_agrees_with_the_snapshot() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let engine = engine(&store, 0.0, 2);
    let outcome = engine
        .distill_hub("bank", "hub", &ctx("bank"), ResolvedRetention::default())
        .unwrap();
    assert_eq!(outcome.status, "distilled");

    let item = store
        .get_memory_item("bank", &outcome.item_id)
        .unwrap()
        .expect("distilled item");
    assert_eq!(item.source_ref.as_deref(), Some(HUB_DISTILL_MARKER));
    assert_ne!(
        item.source_ref.as_deref(),
        Some(HUB_DISTILL_JOB),
        "the written provenance marker is not the maintenance job kind"
    );

    let record = store
        .get_distilled_unit("bank", &outcome.item_id)
        .unwrap()
        .expect("record resolvable by the distilled item id");
    assert_eq!(record.item_id, outcome.item_id);
    assert_eq!(record.bank_id, "bank");
    assert_eq!(record.hub_id, "hub");
    assert_eq!(record.marker, HUB_DISTILL_MARKER);
    assert_eq!(record.record_version, DISTILLED_UNIT_RECORD_VERSION);
    assert_eq!(
        record.distilled_at, item.valid_from,
        "record and item are stamped from one cluster state"
    );
    assert_eq!(record.source_ids.len(), 3, "one entry per neighbour");
    assert_eq!(
        record.claim_ids(),
        snapshot_claim_ids(store.as_ref(), "bank", &outcome.item_id),
        "the record is the claim-set authority; the snapshot projects it"
    );

    // Keyed by the item id, never by the hub the item inherits its subject from.
    assert!(
        store.get_distilled_unit("bank", "hub").unwrap().is_none(),
        "the hub's own id must not resolve a record"
    );
}

#[test]
fn the_record_is_still_resolvable_after_the_store_reopens() {
    let path = scratch_db("am-distill-record");
    let item_id;
    {
        let kms: Arc<dyn clio_store::DekProvider> = Arc::new(LocalDevKms::new());
        let store = Arc::new(SqliteStore::open(&path, kms).expect("open scratch store"));
        seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
        let engine = engine(&store, 0.0, 2);
        let outcome = engine
            .distill_hub("bank", "hub", &ctx("bank"), ResolvedRetention::default())
            .unwrap();
        assert_eq!(outcome.status, "distilled");
        item_id = outcome.item_id;
    }

    // A second open is a second connection over the same file. A fresh key
    // provider proves the read needs no DEK: the record holds ids only.
    let kms: Arc<dyn clio_store::DekProvider> = Arc::new(LocalDevKms::new());
    let reopened = SqliteStore::open(&path, kms).expect("reopen scratch store");
    let record = reopened
        .get_distilled_unit("bank", &item_id)
        .unwrap()
        .expect("record survives the reopen");
    assert_eq!(record.hub_id, "hub");
    assert_eq!(record.marker, HUB_DISTILL_MARKER);
    assert_eq!(record.claim_ids().len(), 4, "hub plus three neighbours");
    drop(reopened);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_second_wave_keeps_one_record_and_the_same_claim_set() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let mut engine = engine(&store, 0.0, 2);
    // Keep the cluster edges so the hub stays eligible for the second wave.
    engine.set_prune_after_distill(false);
    let first = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(first.distilled, vec![DISTILLED.to_owned()]);
    let before = store
        .get_distilled_unit("bank", DISTILLED)
        .unwrap()
        .expect("record after the first wave");

    let second = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    // The known unit is reconciled, not skipped: an unchanged cluster is left
    // exactly as it was, with no revision and no record rewrite.
    assert_eq!(second.reconcile_unchanged, vec!["hub".to_owned()]);
    assert!(second.revised.is_empty(), "revised: {:?}", second.revised);
    let after = store
        .get_distilled_unit("bank", DISTILLED)
        .unwrap()
        .expect("record after the second wave");
    assert_eq!(
        before, after,
        "a wave over an unchanged cluster must not duplicate or rewrite the record"
    );
    // Trail-specific: a no-op wave appends no list at all, and the revision-1
    // list it wrote is still exactly what the first wave recorded.
    assert_eq!(
        after.evidence_history().len(),
        1,
        "only revision 1 has a list; a no-op wave appends nothing"
    );
    assert_eq!(
        before.evidence_history(),
        after.evidence_history(),
        "the recorded list is byte-identical across the no-op wave"
    );
    assert_eq!(after.proof_count(), 4, "hub plus three neighbours");
    assert!(
        store
            .get_memory_item("bank", DISTILLED)
            .unwrap()
            .is_some_and(|item| item.tx_until.is_none()),
        "the unit stays current with no revision row"
    );
}

#[test]
fn a_sibling_sharing_the_hubs_subject_is_untouched() {
    let store = store();
    // The hub and a sibling share one data subject: the shape a `(bank,
    // subject)` lineage lookup would resolve, and close, by mistake.
    for id in ["hub", "sibling", "n1", "n2", "n3"] {
        let mut item = semantic(id, "bank", &format!("{id} core"));
        item.subject_id = "shared-subject".into();
        store.create_memory_item(&item, "agent").unwrap();
    }
    for neighbor in ["n1", "n2", "n3"] {
        store
            .reinforce_assoc_edge("bank", "hub", neighbor, 0, 0.15)
            .expect("reinforce");
    }
    let engine = engine(&store, 0.0, 2);
    let outcome = engine
        .distill_hub("bank", "hub", &ctx("bank"), ResolvedRetention::default())
        .unwrap();
    assert_eq!(outcome.status, "distilled");

    // The record resolves by the distilled item's own id...
    let record = store
        .get_distilled_unit("bank", &outcome.item_id)
        .unwrap()
        .expect("record");
    assert_eq!(record.hub_id, "hub");
    // ...and the sibling sharing the hub's subject is neither closed nor read.
    let sibling = store
        .get_memory_item("bank", "sibling")
        .unwrap()
        .expect("sibling stays readable");
    assert_eq!(sibling.tx_until, None, "the sibling must not be closed");
    assert_eq!(sibling.subject_id, "shared-subject");
    assert!(
        store
            .get_distilled_unit("bank", "sibling")
            .unwrap()
            .is_none(),
        "no record is keyed by the sibling's id"
    );
}

#[test]
fn a_refused_store_write_leaves_no_untracked_unit() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    // Shred the hub's subject key: the store's item insert inside the atomic
    // item+record transaction now refuses. The read path still works, so the
    // failure is the write and not the scan.
    store
        .upsert_subject_key_ref("subj", "local:dead:v1", "shredded")
        .unwrap();
    assert!(
        store.get_memory_item("bank", "hub").unwrap().is_some(),
        "the hub must stay readable so the failure is the write"
    );

    let engine = engine(&store, 0.0, 2);
    let err = engine
        .distill_hub("bank", "hub", &ctx("bank"), ResolvedRetention::default())
        .expect_err("a refused store write must surface as an error");
    assert_eq!(err.code, ErrorCode::ErasedSubject);
    assert!(
        store.get_memory_item("bank", DISTILLED).unwrap().is_none(),
        "no item may be stored when the write fails"
    );
    assert!(
        store
            .get_distilled_unit("bank", DISTILLED)
            .unwrap()
            .is_none(),
        "no record may be stored when the write fails"
    );
}

#[test]
fn an_admission_reject_writes_neither_item_nor_record() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let engine = engine(&store, 1.0, 2);
    let report = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(report.rejected, vec!["hub".to_owned()]);
    assert!(
        store.get_memory_item("bank", DISTILLED).unwrap().is_none(),
        "a rejected distill must not fake a write"
    );
    assert!(
        store
            .get_distilled_unit("bank", DISTILLED)
            .unwrap()
            .is_none(),
        "a rejected distill must not leave a record"
    );
}

#[test]
fn the_distilled_unit_keeps_its_sources_creation_time_in_canonical_form() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let hub = store
        .get_memory_item("bank", "hub")
        .unwrap()
        .expect("seeded hub");
    let engine = engine(&store, 0.0, 2);
    let outcome = engine
        .distill_hub("bank", "hub", &ctx("bank"), ResolvedRetention::default())
        .unwrap();
    assert_eq!(outcome.status, "distilled");

    let item = store
        .get_memory_item("bank", &outcome.item_id)
        .unwrap()
        .expect("distilled item");
    assert_eq!(
        item.created_at, "2026-01-01T00:00:00Z",
        "a distilled unit is created when the cluster it summarizes was, so it keeps the hub's own creation time"
    );
    assert_eq!(item.created_at, hub.created_at, "the hub is the source");
    for stamp in [&item.valid_from, &item.tx_from, &item.updated_at] {
        assert_eq!(
            stamp, "1970-01-01T00:00:00Z",
            "the distillation moment is canonical RFC 3339, never an epoch stamp: {stamp}"
        );
    }
}

#[test]
fn a_time_bounded_selection_places_the_distilled_unit_at_its_sources_time() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let engine = engine(&store, 0.0, 2);
    let outcome = engine
        .distill_hub("bank", "hub", &ctx("bank"), ResolvedRetention::default())
        .unwrap();
    assert_eq!(outcome.status, "distilled");

    let selected = |end: &str| {
        let mut filter = clio_store::ItemInspectFilter::new("bank");
        filter.limit = 200;
        filter.time_end = Some(end.to_owned());
        store
            .inspect_items(&filter)
            .expect("inspect")
            .into_iter()
            .any(|row| row.item_id == outcome.item_id)
    };

    assert!(
        selected("2026-06-01T00:00:00Z"),
        "an upper bound after the source's creation time includes the unit"
    );
    assert!(
        !selected("2025-06-01T00:00:00Z"),
        "an upper bound before the source's creation time excludes it"
    );
}
