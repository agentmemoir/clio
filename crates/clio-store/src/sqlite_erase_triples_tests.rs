// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the SQLite compliance purge of SPO triple rows and their
//! sync-derived payload copies.
//!
//! Split out of `sqlite_erase_triples` so both stay within the source-size limit.

use super::*;
use crate::compliance_store::ComplianceStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::sync_store::{SyncDeadLetterRow, SyncJournalRow, SyncStore};

const NEEDLE: &str = "hunter2Correct";

fn store() -> SqliteStore {
    SqliteStore::open_in_memory(crate::erase_test_support::local_kms()).expect("sqlite")
}

use crate::test_unique::unique;

fn carrier(bank: &str, subject: &str) -> clio_types::MemoryItem {
    clio_types::MemoryItem {
        id: "i-1".into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: clio_types::ItemKind::Episodic,
        category: None,
        episodic_type: Some(clio_types::EpisodicType::Triple),
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.85),
        source_ref: Some("turn".into()),
        snapshot: Some(serde_json::json!({"object": NEEDLE})),
        gist: Some(format!("password is {NEEDLE}")),
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-01".into(),
        tx_until: None,
        created_at: "2024-01-01".into(),
        updated_at: "2024-01-01".into(),
        context: None,
    }
}

fn edge(bank: &str) -> clio_types::TripleRecord {
    clio_types::TripleRecord {
        id: "e-1".into(),
        bank_id: bank.into(),
        item_id: "i-1".into(),
        subject: "svc:db".into(),
        predicate: "password_is".into(),
        object: NEEDLE.into(),
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        valid_from: "2024-01-01".into(),
        valid_until: None,
        tx_from: "2024-01-01".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: "2024-01-01".into(),
        updated_at: None,
    }
}

/// A journal row for the triple, shaped as the sync client packages a closed
/// triple: an `invalidate` mutation whose payload is the triple's plaintext
/// SPO, sealed under the node-local journal DEK.
fn journal_row(bank: &str, edge_id: &str) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: format!("mut-{edge_id}"),
        bank_id: bank.into(),
        entity_kind: "triple".into(),
        entity_id: edge_id.into(),
        op: "invalidate".into(),
        transaction_time: "2024-02-01T00:00:00Z".into(),
        device_id: "dev-1".into(),
        content_hash: "hash-1".into(),
        content_sealed: Some(format!("sealed-payload-{NEEDLE}")),
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2024-02-01T00:00:00Z".into(),
    }
}

fn dead_letter_row(bank: &str, edge_id: &str) -> SyncDeadLetterRow {
    SyncDeadLetterRow {
        sync_mutation_id: format!("dlq-{edge_id}"),
        bank_id: bank.into(),
        reason_code: "malformed".into(),
        detail: "never applied".into(),
        envelope_json: serde_json::json!({
            "entity_kind": "triple",
            "entity_id": edge_id,
            "op": "invalidate",
            "content": {"subject": "svc:db", "predicate": "password_is", "object": NEEDLE},
        })
        .to_string(),
        status: crate::sync_store::DeadLetterStatus::Open,
        first_seen: "2024-02-01T00:00:00Z".into(),
        last_error: Some("boom".into()),
        updated_at: None,
    }
}

fn journal_payload(store: &SqliteStore, bank: &str, edge_id: &str) -> Option<String> {
    store
        .journal_get(bank, &format!("mut-{edge_id}"))
        .expect("journal get")
        .and_then(|r| r.content_sealed)
}

fn dead_letter_envelope(store: &SqliteStore, bank: &str, edge_id: &str) -> String {
    envelope_of(store, bank, &format!("dlq-{edge_id}"))
}

fn envelope_of(store: &SqliteStore, bank: &str, mutation_id: &str) -> String {
    store
        .dead_letter_list(bank, crate::sync_store::DeadLetterStatus::Open, 10)
        .expect("dead letter list")
        .into_iter()
        .find(|r| r.sync_mutation_id == mutation_id)
        .map(|r| r.envelope_json)
        .unwrap_or_default()
}

#[test]
fn purging_a_triple_reports_its_ids_for_the_sync_purge() {
    let s = store();
    let bank = unique("b");
    let subject = unique("subj");
    s.commit_triple_add(&carrier(&bank, &subject), &edge(&bank), true, "actor")
        .expect("triple add");
    s.with_conn(|conn| {
        let tx = conn.transaction().am("test tx").expect("tx");
        let purged = purge_item_triples(&tx, &bank, "i-1").expect("purge");
        assert_eq!(purged.rows, 1);
        assert_eq!(purged.ids, vec!["e-1".to_owned()]);
        // A second purge of the same item has nothing left to remove.
        assert_eq!(
            purge_item_triples(&tx, &bank, "i-1").expect("second purge"),
            PurgedTriples::default()
        );
        // So has one that never carried a triple.
        assert_eq!(
            purge_item_triples(&tx, &bank, "i-missing").expect("missing item"),
            PurgedTriples::default()
        );
        tx.commit().am("test tx commit").expect("commit");
        Ok(())
    })
    .expect("purge run");
    assert!(s.get_triple(&bank, "e-1").unwrap().is_none());
}

#[test]
fn the_erase_blanks_the_sync_journal_and_dead_letter_copies() {
    let s = store();
    let bank = unique("b");
    let subject = unique("subj");
    s.commit_triple_add(&carrier(&bank, &subject), &edge(&bank), true, "actor")
        .expect("triple add");
    s.journal_append(&journal_row(&bank, "e-1"))
        .expect("journal append");
    s.dead_letter_open(&dead_letter_row(&bank, "e-1"))
        .expect("dead letter");
    // The pre-erase state must actually contain the plaintext payload, or this
    // test would pass for the wrong reason.
    assert!(journal_payload(&s, &bank, "e-1").unwrap().contains(NEEDLE));
    assert!(dead_letter_envelope(&s, &bank, "e-1").contains(NEEDLE));

    s.erase_subject(&crate::erase_test_support::spec(
        &bank, &subject, "req-sync",
    ))
    .expect("compliance erase");

    // Both copies keep their bookkeeping rows so sync's replay and ack
    // accounting stay sane, but the payloads are gone.
    let payload = journal_payload(&s, &bank, "e-1");
    assert!(
        payload.is_none(),
        "journal payload must be blanked: {payload:?}"
    );
    assert_eq!(
        s.journal_max_seq(&bank).expect("max seq"),
        1,
        "row survives"
    );
    let envelope = dead_letter_envelope(&s, &bank, "e-1");
    assert!(!envelope.contains(NEEDLE), "envelope must be blanked");
    assert_eq!(envelope, crate::sqlite_erase_purge::ERASED_SYNC_ENVELOPE);
    assert_eq!(
        s.dead_letter_status("dlq-e-1").expect("status"),
        Some(crate::sync_store::DeadLetterStatus::Open),
        "the quarantine row survives"
    );
}

#[test]
fn the_purge_only_touches_the_purged_triples_own_copies() {
    let s = store();
    let bank = unique("b");
    let subject = unique("subj");
    s.commit_triple_add(&carrier(&bank, &subject), &edge(&bank), true, "actor")
        .expect("triple add");
    // Another triple's journal row, and a row of a different entity kind. Both
    // must survive: the purge is scoped to the ids of the triples it deleted.
    // (A `persona` row would not be a fair probe here — the persona purge is
    // bank-scoped and blanks those on any verified erasure, by design.)
    let mut other = journal_row(&bank, "e-1");
    other.sync_mutation_id = "mut-other-triple".into();
    other.entity_id = "e-other".into();
    s.journal_append(&other).expect("journal append");
    let mut belief = journal_row(&bank, "e-1");
    belief.sync_mutation_id = "mut-belief".into();
    belief.entity_kind = "belief".into();
    belief.content_sealed = Some("belief-payload".into());
    s.journal_append(&belief).expect("journal append");
    let mut other_dead = dead_letter_row(&bank, "e-1");
    other_dead.sync_mutation_id = "dlq-other".into();
    other_dead.envelope_json = serde_json::json!({
        "entity_kind": "triple",
        "entity_id": "e-other",
        "content": {"object": "unrelated"},
    })
    .to_string();
    s.dead_letter_open(&other_dead).expect("dead letter");

    s.erase_subject(&crate::erase_test_support::spec(
        &bank,
        &subject,
        "req-scope",
    ))
    .expect("compliance erase");

    assert_eq!(
        journal_payload(&s, &bank, "e-1"),
        None,
        "the purged triple's own journal row is blanked"
    );
    let other_payload = s
        .journal_get(&bank, "mut-other-triple")
        .expect("journal get")
        .expect("row survives")
        .content_sealed;
    assert!(
        other_payload.unwrap().contains(NEEDLE),
        "another triple's copy stays"
    );
    let belief_payload = s
        .journal_get(&bank, "mut-belief")
        .expect("journal get")
        .expect("row survives")
        .content_sealed;
    assert_eq!(belief_payload.as_deref(), Some("belief-payload"));
    assert!(
        envelope_of(&s, &bank, "dlq-other").contains("unrelated"),
        "another triple's envelope stays"
    );
}

#[test]
fn purging_sync_copies_with_no_triples_is_a_noop() {
    let s = store();
    let bank = unique("b");
    s.with_conn(|conn| {
        let tx = conn.transaction().am("test tx").expect("tx");
        assert_eq!(purge_sync_copies(&tx, &bank, &[]).expect("empty purge"), 0);
        tx.commit().am("test tx commit").expect("commit");
        Ok(())
    })
    .expect("purge run");
}

#[test]
fn envelope_id_parsing_tolerates_junk() {
    assert_eq!(
        envelope_entity_id(r#"{"entity_id":"e-9"}"#).as_deref(),
        Some("e-9")
    );
    assert_eq!(envelope_entity_id("not json"), None);
    assert_eq!(envelope_entity_id(r#"{"entity_id":7}"#), None);
    assert_eq!(
        crate::sqlite_erase_purge::envelope_entity_kind("not json"),
        None
    );
}

#[test]
fn the_purge_is_scoped_to_its_bank() {
    // `bank_id` and `item_id` are both bound: a row in another bank with the
    // same item id must survive.
    let s = store();
    let bank = unique("b");
    let other = unique("b");
    let subject = unique("subj");
    let mut keep = carrier(&other, &subject);
    keep.id = "i-1".into();
    let mut keep_edge = edge(&other);
    keep_edge.item_id = "i-1".into();
    s.commit_triple_add(&keep, &keep_edge, true, "actor")
        .expect("triple add");

    s.with_conn(|conn| {
        let tx = conn.transaction().am("test tx").expect("tx");
        let purged = purge_item_triples(&tx, &bank, "i-1").expect("purge");
        assert_eq!(purged.rows, 0, "another bank's row is not ours");
        tx.commit().am("test tx commit").expect("commit");
        Ok(())
    })
    .expect("purge run");
    assert!(s.get_triple(&other, "e-1").unwrap().is_some());
}
