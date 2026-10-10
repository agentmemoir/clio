// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the pull path's handling of an
//! opaque identifier handle: that a sealed row applies exactly as a plaintext
//! row does and passes the apply matrix's own identifier checks, that an
//! identical redelivery stays a duplicate, that the conflict sample an operator
//! reads shows the real identifier, and that a row whose payload carries no
//! identifier is refused rather than applied with the handle in its place.
//!
//! ## Owns
//! - End-to-end apply, idempotency, conflict reporting, and refusal behaviour
//!   for rows that arrived carrying a handle
//!
//! ## Does not own
//! - What the relay can read off its stored bytes
//!   (`client_wire_blindness_tests`)
//! - Handle collisions and the mutation-id derivation
//!   (`client_feed_package_handle_tests`)
//! - The resolution logic itself (`handle_resolution_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the pull path changes; they never change it.
//!
//! Keep this module focused on pull-path handle resolution tests.

use crate::client::client_tests::{cfg, serve, sqlite};
use crate::client::{ack_skip, pull, push, sync_status};
use crate::crypto::{SyncCipher, SyncCipherKey};
use crate::http::post_json;
use crate::protocol::{
    ENTITY_KIND_TASK, OP_UPSERT, PushAck, SyncContent, SyncMutation, SyncPayload,
};
use clio_store::{
    AssocEdgeRecord, AssocKind, DeadLetterStatus, HistoryStore, PersonaStore, Store, SyncStore,
};
use clio_types::{
    EpisodicType, EpistemicKind, ItemKind, MemoryItem, PersonaStableEntry, TaskRecord,
};
use serde_json::json;

const TASK_ID: &str = "synthetic-task-id-42";
const ATTR_NAME: &str = "synthetic_attr_name";
const RELATIONSHIP: &str = "synthetic_relationship_label";

fn carrier(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Triple),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.85),
        source_ref: Some("turn".into()),
        snapshot: Some(json!({"object": "obj"})),
        gist: Some("carrier".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

fn task_record(bank: &str) -> TaskRecord {
    TaskRecord {
        id: "task-row-1".into(),
        bank_id: bank.into(),
        item_id: "car-task".into(),
        task_id: TASK_ID.into(),
        subject_id: "subj".into(),
        version: 1,
        definition: "do the thing".into(),
        steps: vec!["step".into()],
        status: "open".into(),
        evidence_ref: None,
        context: None,
        admission_score: 0.85,
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn seed_task(a: &std::sync::Arc<clio_store::SqliteStore>, bank: &str) {
    let mut item = carrier("car-task", bank);
    item.episodic_type = Some(EpisodicType::Task);
    item.gist = Some("do the thing".into());
    a.commit_task_upsert(&item, &task_record(bank), "agent")
        .expect("a task");
}

/// A sealed task row applied on a peer holding the key: the task is
/// materialized under its real task id, which is only possible because the
/// handle was resolved before apply and the apply matrix's own identifier check
/// then compared a real identifier against a real identifier.
#[test]
fn a_sealed_task_row_applies_under_its_real_identifier() {
    let (server, srv, _kms) = serve(&["br"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let key = SyncCipherKey::from_secret("resolve-task-key");
    let cfg_a = cfg("dev-a", &a, &addr, vec![key.clone()]);
    let cfg_b = cfg("dev-b", &b, &addr, vec![key]);
    seed_task(&a, "br");
    push(&cfg_a, &["br".to_owned()]).expect("push");
    let relayed = srv.journal_list_since("br", 0, 10).expect("server journal");
    let handle = relayed
        .iter()
        .find(|r| r.entity_kind == ENTITY_KIND_TASK)
        .expect("the task row reaches the relay")
        .entity_id
        .clone();
    assert_ne!(handle, TASK_ID, "the wire identifier is a handle");

    pull(&cfg_b, &["br".to_owned()]).expect("pull");
    let got = b
        .get_task("br", TASK_ID)
        .expect("task get")
        .expect("the task is stored under its real id, so the apply check passed");
    assert_eq!(got.definition, "do the thing");
    assert_eq!(
        b.dead_letter_count("br", DeadLetterStatus::Open).unwrap(),
        0,
        "an encrypted pull must not dead-letter a resolvable row"
    );
    // Nothing opaque reached the store's business-identifier column.
    let journal = b.journal_list_since("br", 0, 10).expect("local journal");
    let applied = journal
        .iter()
        .find(|r| r.entity_kind == ENTITY_KIND_TASK)
        .expect("the applied row is journaled locally");
    assert_eq!(
        applied.entity_id, TASK_ID,
        "the local journal records the resolved identifier"
    );
    server.stop();
}

/// Redelivering the identical sealed row is a duplicate, not a second version:
/// the idempotency key is derived from the real identifier and both sides
/// compare the same wire digest, so nothing accumulates.
#[test]
fn a_redelivered_sealed_row_stays_a_duplicate() {
    let (server, srv, _kms) = serve(&["br"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let key = SyncCipherKey::from_secret("resolve-idem-key");
    let cfg_a = cfg("dev-a", &a, &addr, vec![key.clone()]);
    let cfg_b = cfg("dev-b", &b, &addr, vec![key]);
    seed_task(&a, "br");
    push(&cfg_a, &["br".to_owned()]).expect("push");
    pull(&cfg_b, &["br".to_owned()]).expect("pull");
    assert_eq!(b.task_history("br", TASK_ID, 10).expect("history").len(), 1);

    // A second identical row on the relay, as a redelivery would look.
    let seq = srv.journal_max_seq("br").unwrap();
    push(&cfg_a, &["br".to_owned()]).expect("re-push");
    pull(&cfg_b, &["br".to_owned()]).expect("re-pull");
    assert_eq!(
        srv.journal_max_seq("br").unwrap(),
        seq,
        "the sender's own re-push adds no relay row"
    );
    assert_eq!(
        b.task_history("br", TASK_ID, 10).expect("history").len(),
        1,
        "no second version accumulates"
    );
    assert_eq!(
        b.get_task("br", TASK_ID)
            .expect("get")
            .expect("row")
            .version,
        1
    );
    server.stop();
}

/// An operator reading the status report sees the real task id in a conflict
/// sample, not an opaque handle.
#[test]
fn a_conflict_sample_shows_the_real_identifier_not_a_handle() {
    let (server, srv, _kms) = serve(&["br"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let key = SyncCipherKey::from_secret("resolve-conflict-key");
    let cfg_a = cfg("dev-a", &a, &addr, vec![key.clone()]);
    let cfg_b = cfg("dev-b", &b, &addr, vec![key]);
    seed_task(&a, "br");
    // The peer holds its own lower version of the same task, so the incoming
    // row wins and nothing conflicts. Seed a conflicting higher version instead:
    // a local row the incoming row will lose to.
    let mut local_item = carrier("car-task", "br");
    local_item.episodic_type = Some(EpisodicType::Task);
    local_item.gist = Some("local newer".into());
    let mut local = task_record("br");
    local.version = 2;
    local.definition = "newer local".into();
    b.commit_task_upsert(&local_item, &local, "agent")
        .expect("local task");
    let seq = b.journal_max_seq("br").unwrap();
    push(&cfg_a, &["br".to_owned()]).expect("push");
    pull(&cfg_b, &["br".to_owned()]).expect("pull");

    let report = sync_status(&cfg_b, &["br".to_owned()]).expect("status");
    let bank = &report.banks[0];
    assert!(
        bank.conflicts > 0,
        "a lower incoming version against a higher local one conflicts"
    );
    let handle = srv
        .journal_list_since("br", 0, 10)
        .expect("relay journal")
        .iter()
        .find(|r| r.entity_kind == ENTITY_KIND_TASK)
        .expect("the task row reaches the relay")
        .entity_id
        .clone();
    assert_ne!(handle, TASK_ID, "the relay saw a handle");
    assert!(
        bank.conflict_sample.iter().any(|s| s.starts_with(TASK_ID)),
        "the sample names the real task id: {:?}",
        bank.conflict_sample
    );
    assert!(
        !bank.conflict_sample.iter().any(|s| s.contains(&handle)),
        "no handle stands in for the identifier: {:?}",
        bank.conflict_sample
    );
    assert_eq!(
        seq, 0,
        "the losing row is not applied, so nothing is journaled"
    );
    server.stop();
}

/// A sealed row whose payload does not carry the identifier its handle claims
/// is refused: the cursor freezes, the row is dead-lettered with a reason that
/// names the kind and the bank, and nothing is applied.
#[test]
fn a_row_whose_payload_lacks_its_identifier_is_refused() {
    let (server, srv, _kms) = serve(&["br"]);
    let addr = server.addr().to_owned();
    let b = sqlite();
    let key = SyncCipherKey::from_secret("resolve-refuse-key");
    let cfg_b = cfg("dev-b", &b, &addr, vec![key.clone()]);

    // Hand-push a sealed task row whose payload has no task record, so the
    // handle has nothing in the opened content to resolve against.
    let plain = json!({"item": carrier("car-x", "br")});
    let sealed = key
        .seal(&serde_json::to_vec(&plain).expect("plain bytes"))
        .expect("seal");
    // The idempotency key a current writer would produce for this row, so the
    // fixture is the real wire shape rather than a hand-rolled one.
    let mutation_id = crate::keyed_roles::mutation_id(Some(&key), TASK_ID, "2026-01-01", OP_UPSERT);
    let mutation = SyncMutation {
        sync_mutation_id: mutation_id,
        bank: "br".into(),
        entity_kind: ENTITY_KIND_TASK.into(),
        entity_id: "3f2a9c8b7d6e5f40123456789abcdef".into(),
        op: OP_UPSERT.into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-hand".into(),
        content: SyncContent::Cipher(SyncCipher {
            key_id: Some(key.key_id()),
            ..sealed
        }),
        content_hash: clio_store::sha256_hex(b"am-sync-hash", "anything"),
        admission_receipt: None,
    };
    let ack: PushAck = post_json(
        &addr,
        "/sync/push",
        Some(crate::client::client_tests::TOKEN),
        &serde_json::to_value(&SyncPayload {
            device_id: "dev-hand".into(),
            bank: "br".into(),
            mutations: vec![mutation],
        })
        .expect("payload"),
    )
    .expect("hand push");
    assert!(ack.rejected.is_empty(), "the relay stores it verbatim");

    pull(&cfg_b, &["br".to_owned()]).expect("pull must not abort");
    assert_eq!(
        b.dead_letter_count("br", DeadLetterStatus::Open).unwrap(),
        1,
        "the unresolvable row is dead-lettered"
    );
    let dlq = b
        .dead_letter_list("br", DeadLetterStatus::Open, 10)
        .expect("dead letters");
    assert_eq!(dlq[0].reason_code, "malformed");
    assert!(
        dlq[0].detail.contains("task") && dlq[0].detail.contains("br"),
        "the refusal names the kind and the bank: {}",
        dlq[0].detail
    );
    assert!(
        !dlq[0].detail.contains("3f2a9c8b"),
        "the refusal never echoes the wire handle: {}",
        dlq[0].detail
    );
    assert!(
        b.get_task("br", TASK_ID).expect("get").is_none(),
        "nothing was applied"
    );
    // The cursor froze, so the operator decides what happens to the row.
    assert!(
        ack_skip(&cfg_b, &dlq[0].sync_mutation_id, "2026-01-02T00:00:00Z").expect("ack"),
        "the operator can discard the row"
    );
    assert!(
        srv.journal_max_seq("br").unwrap() > 0,
        "the relay row is untouched"
    );
    server.stop();
}

/// A sealed persona row applies under its real attribute key, so the persona
/// apply path's own identifier check sees a real identifier too.
#[test]
fn a_sealed_persona_row_applies_under_its_real_attribute_key() {
    let (server, srv, _kms) = serve(&["br"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let key = SyncCipherKey::from_secret("resolve-persona-key");
    let cfg_a = cfg("dev-a", &a, &addr, vec![key.clone()]);
    let cfg_b = cfg("dev-b", &b, &addr, vec![key]);
    a.put_stable_entry(
        &PersonaStableEntry {
            bank_id: "br".into(),
            key: ATTR_NAME.into(),
            value: "terse".into(),
            confidence: None,
            source: "explicit".into(),
            source_ref: None,
            context: None,
            admission_score: 0.5,
            as_of: "2026-01-01".into(),
            valid_until: None,
        },
        "seed",
    )
    .expect("persona");
    push(&cfg_a, &["br".to_owned()]).expect("push");
    assert!(
        !srv.journal_list_since("br", 0, 10).expect("journal")[0]
            .entity_id
            .contains(ATTR_NAME),
        "the relay saw a handle"
    );
    pull(&cfg_b, &["br".to_owned()]).expect("pull");
    let doc = b.get_persona_document("br").expect("persona document");
    assert_eq!(doc.stable.len(), 1);
    assert_eq!(
        doc.stable[0].key, ATTR_NAME,
        "the peer applied it under the real attribute key"
    );
    server.stop();
}

/// A sealed explicit-edge row resolves to the edge id, so the edge is stored
/// under its real id and both endpoint items are still matched locally.
#[test]
fn a_sealed_explicit_edge_row_resolves_to_the_edge_id() {
    let (server, srv, _kms) = serve(&["br"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let b = sqlite();
    let key = SyncCipherKey::from_secret("resolve-edge-key");
    let cfg_a = cfg("dev-a", &a, &addr, vec![key.clone()]);
    let cfg_b = cfg("dev-b", &b, &addr, vec![key]);
    for id in ["i-1", "i-2"] {
        a.create_memory_item(&carrier(id, "br"), "seed")
            .expect("endpoint");
        b.create_memory_item(&carrier(id, "br"), "seed")
            .expect("endpoint");
    }
    let edge_id = clio_store::explicit_id("br", "i-1", "i-2", RELATIONSHIP);
    a.put_assoc_edge(&AssocEdgeRecord {
        id: edge_id.clone(),
        bank_id: "br".into(),
        source_id: "i-1".into(),
        target_id: "i-2".into(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some(RELATIONSHIP.into()),
        weight: 0.5,
        last_updated_at: "2026-01-01T02:00:00Z".into(),
    })
    .expect("edge");
    push(&cfg_a, &["br".to_owned()]).expect("push");
    let relayed = srv.journal_list_since("br", 0, 10).expect("journal");
    assert!(
        !relayed.iter().any(|r| stored_row(r).contains(RELATIONSHIP)),
        "the relationship label stayed off the relay"
    );
    pull(&cfg_b, &["br".to_owned()]).expect("pull");
    let edges = b.list_assoc_edges("br", None).expect("edges");
    assert_eq!(edges.len(), 1);
    assert_eq!(
        edges[0].id, edge_id,
        "the edge is stored under its real id, not the handle"
    );
    assert_eq!(
        edges[0].relationship.as_deref(),
        Some(RELATIONSHIP),
        "the sealed payload still carries the relationship for the local row"
    );
    server.stop();
}

/// Every string a relay could read off one stored row.
fn stored_row(row: &clio_store::SyncJournalRow) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        row.entity_kind,
        row.entity_id,
        row.op,
        row.transaction_time,
        row.device_id,
        row.content_sealed.as_deref().unwrap_or_default(),
    )
}
