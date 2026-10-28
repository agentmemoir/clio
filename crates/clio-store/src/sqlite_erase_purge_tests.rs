// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite compliance-erasure purge SQL: subject
//! selection, persona-context detection and purge across its live and
//! sync-derived copies, and the per-bank derived-row sweep with its
//! hub-distilled referrers.
//!
//! ## Owns
//! - The selection/tombstone assertions
//! - The persona-context eligibility and purge assertions over stable,
//!   preference, journal, and dead-letter copies
//! - The `purge_bank` sweep assertions including the MemTree parent chain,
//!   the missing-parent break, and hub-distilled referrer purge
//!
//! ## Does not own
//! - Production behaviour
//! - Shared erasure fixtures (`erase_test_support`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the SQLite erasure purge SQL.

use super::*;
use crate::erase_test_support::{link_hub, local_kms, memory_item, seed, seed_ghost, unique};
use crate::index_store::IndexStore;
use crate::model::{AssocEdgeRecord, AssocKind, EMBEDDING_DIMS, VectorSlot};
use crate::persona_store::PersonaStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{PersonaPreference, PersonaStableEntry};
use std::sync::Arc;

const T0: &str = "2026-06-01T00:00:00Z";

fn stable_entry(bank: &str, context: Option<&str>) -> PersonaStableEntry {
    PersonaStableEntry {
        bank_id: bank.into(),
        key: "stable-key".into(),
        value: "stable value".into(),
        confidence: Some(0.9),
        source: "explicit".into(),
        source_ref: Some("turn_1".into()),
        context: context.map(str::to_owned),
        admission_score: 0.5,
        as_of: T0.into(),
        valid_until: None,
    }
}

fn preference(bank: &str, context: Option<&str>) -> PersonaPreference {
    PersonaPreference {
        bank_id: bank.into(),
        key: "pref-key".into(),
        value: 0.5,
        trend: 0.1,
        confidence: Some(0.8),
        source: "explicit".into(),
        source_ref: Some("turn_1".into()),
        context: context.map(str::to_owned),
        admission_score: 0.5,
        as_of: T0.into(),
    }
}

fn open() -> SqliteStore {
    let kms: Arc<dyn crate::dek::DekProvider> = local_kms();
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

#[test]
fn selection_orders_items_lists_every_bank_and_overwrites_the_tombstone() {
    let store = open();
    let bank1 = unique("purge-b1");
    let bank2 = unique("purge-b2");
    let subject = unique("subject");
    let a = unique("a");
    let b = unique("b");
    let c = unique("c");
    store
        .create_memory_item(&memory_item(&bank1, &a, &subject), "tester")
        .expect("create a");
    store
        .create_memory_item(&memory_item(&bank1, &b, &subject), "tester")
        .expect("create b");
    store
        .create_memory_item(&memory_item(&bank2, &c, &subject), "tester")
        .expect("create c");
    store
        .create_memory_item(
            &memory_item(&bank1, &unique("unrelated"), &unique("other-subject")),
            "tester",
        )
        .expect("create unrelated");

    store
        .with_conn(|conn| {
            let tx = conn.transaction().expect("tx");
            let rows = select_items(&tx, &bank1, &subject).expect("select items");
            let ids: Vec<&str> = rows.iter().map(|r| r.item_id.as_str()).collect();
            assert_eq!(ids, vec![a.as_str(), b.as_str()], "items must be id-ordered");
            let subject_banks = select_subject_banks(&tx, &subject).expect("select banks");
            assert_eq!(
                subject_banks,
                vec![bank1.clone(), bank2.clone()],
                "{subject_banks:?}"
            );
            write_persona_erase_tombstone(&tx, &bank1, T0).expect("tombstone");
            write_persona_erase_tombstone(&tx, &bank1, "2026-06-02T00:00:00Z")
                .expect("tombstone overwrite");
            let n: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM persona_erase_tombstones WHERE bank_id = ?1",
                    params![bank1],
                    |r| r.get(0),
                )
                .expect("tombstone count");
            assert_eq!(n, 1, "the tombstone must overwrite, not duplicate");
            Ok(())
        })
        .expect("selection transaction");
}

#[test]
#[allow(clippy::too_many_lines)]
fn persona_context_detection_and_purge_cover_every_copy() {
    let store = open();
    let live = unique("purge-persona-live");
    let dl_only = unique("purge-persona-dl");
    let item_dl = unique("purge-persona-item-dl");
    let empty = unique("purge-persona-none");

    store
        .put_stable_entry(&stable_entry(&live, Some("stable context")), "tester")
        .expect("stable entry");
    store
        .put_preference_state(&preference(&live, Some("pref context")), "tester")
        .expect("preference");
    store
        .with_conn(|conn| {
            let tx = conn.transaction().expect("tx");
            tx.execute(
                "INSERT INTO sync_mutations
                 (bank_id, seq, sync_mutation_id, entity_kind, entity_id, op, transaction_time,
                  device_id, content_hash, content_sealed, encrypted, created_at)
                 VALUES (?1, 0, ?2, 'persona', 'pe', 'upsert', ?3, 'dev', 'hash', 'sealed-body', 1, ?3)",
                params![live, unique("mut"), T0],
            )
            .expect("journal seed");
            tx.execute(
                "INSERT INTO sync_dead_letter
                 (sync_mutation_id, bank_id, reason_code, detail, envelope_json, first_seen)
                 VALUES (?1, ?2, 'reject', 'd', ?3, ?4)",
                params![unique("dl-p"), live, r#"{"entity_kind":"persona"}"#, T0],
            )
            .expect("persona dead letter");
            tx.execute(
                "INSERT INTO sync_dead_letter
                 (sync_mutation_id, bank_id, reason_code, detail, envelope_json, first_seen)
                 VALUES (?1, ?2, 'reject', 'd', ?3, ?4)",
                params![unique("dl-i"), live, r#"{"entity_kind":"item"}"#, T0],
            )
            .expect("item dead letter");
            tx.execute(
                "INSERT INTO sync_dead_letter
                 (sync_mutation_id, bank_id, reason_code, detail, envelope_json, first_seen)
                 VALUES (?1, ?2, 'reject', 'd', ?3, ?4)",
                params![unique("dl-p2"), dl_only, r#"{"entity_kind":"persona"}"#, T0],
            )
            .expect("dead-letter-only persona");
            tx.execute(
                "INSERT INTO sync_dead_letter
                 (sync_mutation_id, bank_id, reason_code, detail, envelope_json, first_seen)
                 VALUES (?1, ?2, 'reject', 'd', ?3, ?4)",
                params![unique("dl-i2"), item_dl, r#"{"entity_kind":"item"}"#, T0],
            )
            .expect("dead-letter-only item");
            tx.commit().am("test commit")
        })
        .expect("persona seeds");

    store
        .with_conn(|conn| {
            let tx = conn.transaction().expect("tx");
            assert!(
                bank_has_persona_context(&tx, &live).expect("live probe"),
                "stable, preference, journal, and dead-letter copies all count"
            );
            assert!(
                bank_has_persona_context(&tx, &dl_only).expect("dead-letter probe"),
                "a bank whose only persona content is a dead-letter envelope is persona-bearing"
            );
            assert!(
                !bank_has_persona_context(&tx, &item_dl).expect("item-envelope probe"),
                "a non-persona dead-letter envelope is not persona content"
            );
            assert!(
                !bank_has_persona_context(&tx, &empty).expect("empty probe"),
                "a bank with nothing must probe false"
            );
            assert_eq!(
                envelope_entity_kind(r#"{"entity_kind":"persona"}"#).as_deref(),
                Some("persona")
            );
            assert_eq!(envelope_entity_kind("{}"), None);
            assert_eq!(envelope_entity_kind("not json"), None);
            Ok(())
        })
        .expect("probes");

    let purged = store
        .with_conn(|conn| {
            let tx = conn.transaction().expect("tx");
            let n = purge_persona_context(&tx, &live)?;
            tx.commit().am("test commit")?;
            Ok(n)
        })
        .expect("persona purge");
    assert_eq!(purged, 4, "stable, preference, journal, and dead letter: {purged}");

    let doc = store.get_persona_document(&live).expect("persona doc");
    assert!(doc.stable[0].context.is_none(), "the stable context must be gone");
    assert!(
        doc.preferences.iter().all(|p| p.context.is_none()),
        "the preference context must be gone"
    );
    store
        .with_conn(|conn| {
            let sealed: Option<String> = conn
                .query_row(
                    "SELECT content_sealed FROM sync_mutations WHERE bank_id = ?1",
                    params![live],
                    |r| r.get(0),
                )
                .expect("journal read");
            assert_eq!(sealed, None, "the sealed journal payload must be blanked");
            let encrypted: i64 = conn
                .query_row(
                    "SELECT encrypted FROM sync_mutations WHERE bank_id = ?1",
                    params![live],
                    |r| r.get(0),
                )
                .expect("journal flag read");
            assert_eq!(encrypted, 0, "the journal row must read as plaintext-cleared");
            let (blanked, kept): (Option<String>, String) = conn
                .query_row(
                    "SELECT MAX(CASE WHEN envelope_json = ?2 THEN sync_mutation_id END),
                            MAX(CASE WHEN envelope_json <> ?2 THEN envelope_json END)
                     FROM sync_dead_letter WHERE bank_id = ?1",
                    params![live, ERASED_SYNC_ENVELOPE],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .expect("dead letter read");
            assert!(blanked.is_some(), "the persona envelope must be blanked");
            assert_eq!(
                kept, r#"{"entity_kind":"item"}"#,
                "the item envelope must be untouched"
            );
            Ok(())
        })
        .expect("post-purge reads");

    let repurged = store
        .with_conn(|conn| {
            let tx = conn.transaction().expect("tx");
            let n = purge_persona_context(&tx, &live)?;
            tx.commit().am("test commit")?;
            Ok(n)
        })
        .expect("repeat purge");
    assert_eq!(repurged, 0, "a second purge must find nothing: {repurged}");
}

#[test]
fn purge_bank_sweeps_the_subject_and_its_hub_distilled_referrers() {
    let store = open();
    let bank = unique("purge-bank");
    let subject = unique("subject");
    let (a, parent) = seed(&store, &bank, &subject);
    let hub = {
        // The hub-distilled referrer belongs to another subject: the fan-out
        // exists to purge derivatives that a subject erasure drags in.
        let other_subject = unique("other-subject");
        let hub = unique("hub-item");
        store
            .create_memory_item(&memory_item(&bank, &hub, &other_subject), "tester")
            .expect("create hub");
        hub
    };
    link_hub(&store, &bank, &hub, &a);
    // A reversed hub-distill edge covers the source-side match, and both edges
    // naming the same referrer cover the dedupe.
    store
        .put_assoc_edge(&AssocEdgeRecord {
            id: unique("rev-hub-edge"),
            bank_id: bank.clone(),
            source_id: a.clone(),
            target_id: hub.clone(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some(HUB_DISTILL_EDGE_RELATIONSHIP.into()),
            weight: 0.5,
            last_updated_at: T0.into(),
        })
        .expect("reversed hub edge");
    store
        .upsert_vector(&VectorSlot {
            item_id: hub.clone(),
            bank_id: bank.clone(),
            model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
            embedding: vec![0.0; EMBEDDING_DIMS],
        })
        .expect("hub vector");
    IndexStore::put_lexical_doc(&store, &bank, &hub, "hub text").expect("hub lexical doc");
    let ghost = seed_ghost(&store, &bank, &subject);

    let out = store
        .with_conn(|conn| {
            let tx = conn.transaction().expect("tx");
            let items = select_items(&tx, &bank, &subject).expect("select items");
            let out = purge_bank(&tx, &bank, &items)?;
            tx.commit().am("test commit")?;
            Ok(out)
        })
        .expect("purge_bank");

    assert_eq!(out.vectors, 2, "the subject item and the hub: {out:?}");
    assert_eq!(out.edges, 3, "the explicit edge plus both hub edges: {out:?}");
    assert_eq!(
        out.memtree, 3,
        "the leaf with its parent plus the ghost leaf: {out:?}"
    );
    assert_eq!(
        out.derived_items,
        vec![hub.clone()],
        "the hub referrer must be found once: {out:?}"
    );

    let node = store
        .get_memtree_node(&bank, &parent)
        .expect("parent read")
        .expect("the parent node survives");
    assert!(node.dirty, "the parent chain must be dirtied: {node:?}");
    assert!(
        node.summary_gist.is_none(),
        "the parent aggregate gist must be gone: {node:?}"
    );
    store
        .with_conn(|conn| {
            let naked: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM items
                     WHERE id = ?1 AND content_ciphertext IS NOT NULL",
                    params![hub],
                    |r| r.get(0),
                )
                .expect("hub content read");
            assert_eq!(naked, 0, "the derived item's sealed content must be gone");
            let pending: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM index_pending WHERE item_id = ?1",
                    params![hub],
                    |r| r.get(0),
                )
                .expect("hub outbox read");
            assert_eq!(pending, 0, "the derived item's outbox row must be gone");
            Ok(())
        })
        .expect("post-purge reads");
    assert!(
        IndexStore::lexical_search(&store, &bank, "hub", &[], 5)
            .expect("lexical after purge")
            .is_empty(),
        "the derived item's lexical copy must be gone"
    );
    let _ = ghost;
}