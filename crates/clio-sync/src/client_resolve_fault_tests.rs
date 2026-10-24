// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two failure modes an encrypted pull must
//! not have moved: a shredded subject still dead-letters with the erasure
//! reason rather than being applied, and a row carrying an algorithm this build
//! does not implement is still refused unchanged.
//!
//! ## Owns
//! - Failure-mode coverage for the encrypted pull path
//!
//! ## Does not own
//! - Handle resolution and its refusal (`client_resolve_tests`)
//! - What the relay can read off its stored bytes
//!   (`client_wire_blindness_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the erasure ordering or the algorithm refusal changes;
//! they never change it.
//!
//! Keep this module focused on encrypted-pull failure modes.

use crate::client::client_tests::{TOKEN, cfg, item, serve, sqlite};
use crate::client::pull;
use crate::client_wire_fixtures::TASK_ID;
use crate::crypto::SyncCipherKey;
use crate::http::post_json;
use crate::protocol::{
    ENTITY_KIND_ITEM, ENTITY_KIND_TASK, OP_UPSERT, PushAck, SyncContent, SyncMutation, SyncPayload,
};
use clio_store::{DeadLetterStatus, HistoryStore, Store, SyncStore};
use serde_json::json;
use std::sync::Arc;

type TestStore = Arc<clio_store::SqliteStore>;

/// Push one mutation the caller shaped by hand, so a test controls the wire
/// shape exactly.
fn hand_push(addr: &str, mutation: SyncMutation) {
    let ack: PushAck = post_json(
        addr,
        "/sync/push",
        Some(TOKEN),
        &serde_json::to_value(&SyncPayload {
            device_id: mutation.device_id.clone(),
            bank: mutation.bank.clone(),
            mutations: vec![mutation],
        })
        .expect("payload"),
    )
    .expect("hand push");
    assert!(ack.rejected.is_empty(), "the relay stores the row verbatim");
}

fn mutation_id(kind_id: &str, stamp: &str, op: &str) -> String {
    clio_store::sha256_hex(
        crate::keyed_roles::published_mutation_id_salt(),
        &format!("{kind_id}|{stamp}|{op}"),
    )
}

/// A sealed row for a subject that was shredded still dead-letters with the
/// erasure reason: the resolution step reads the identifier out of the payload
/// and does not move the erasure check, which runs inside apply.
#[test]
fn a_sealed_row_for_a_shredded_subject_still_dead_letters() {
    let (server, _srv, _kms) = serve(&["bs"]);
    let addr = server.addr().to_owned();
    let b: TestStore = sqlite();
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("shred-key"));
    let cfg_b = cfg("dev-b", &b, &addr, vec![key.clone()]);
    b.upsert_subject_key_ref("subj-gone", "shredded", "shredded")
        .expect("shred marker");

    let fixture = item(
        "bs",
        "i-res",
        "subj-gone",
        "2026-01-01T00:00:00Z",
        "resurrected",
    );
    let plain = serde_json::to_value(&fixture).expect("serialize the item");
    let mutation = crate::envelope_fixtures::seal_content_for(
        SyncMutation {
            sync_mutation_id: mutation_id("i-res", &fixture.tx_from, OP_UPSERT),
            bank: "bs".into(),
            entity_kind: ENTITY_KIND_ITEM.into(),
            entity_id: fixture.id.clone(),
            op: OP_UPSERT.into(),
            transaction_time: fixture.tx_from.clone(),
            device_id: "dev-hand".into(),
            content: SyncContent::Plain(serde_json::Value::Null),
            content_hash: clio_store::sha256_hex(
                crate::keyed_roles::published_digest_salt(),
                "anything",
            ),
            admission_receipt: None,
        },
        &key,
        &plain,
    );
    hand_push(&addr, mutation);
    pull(&cfg_b, &["bs".to_owned()]).expect("pull");
    let open = b
        .dead_letter_list("bs", DeadLetterStatus::Open, 10)
        .expect("dead letters");
    assert_eq!(open.len(), 1);
    assert_eq!(
        open[0].reason_code, "erased_subject",
        "the erasure ordering did not move"
    );
    assert!(
        b.get_memory_item("bs", "i-res").expect("get").is_none(),
        "nothing was applied"
    );
    server.stop();
}

/// A row whose algorithm this build does not implement is refused with the same
/// message as before, so the refusal is unchanged by anything this phase did.
#[test]
fn an_unknown_algorithm_on_an_encrypted_row_is_still_refused() {
    let (server, _srv, _kms) = serve(&["bs"]);
    let addr = server.addr().to_owned();
    let b: TestStore = sqlite();
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("alg-key"));
    let cfg_b = cfg("dev-b", &b, &addr, vec![key.clone()]);

    let payload = json!({"task": {"task_id": TASK_ID, "id": "row-1"}});
    let mut mutation = crate::envelope_fixtures::seal_content_for(
        SyncMutation {
            sync_mutation_id: mutation_id(TASK_ID, "2026-01-01T00:00:00Z", OP_UPSERT),
            bank: "bs".into(),
            entity_kind: ENTITY_KIND_TASK.into(),
            entity_id: "3f2a9c8b7d6e5f40123456789abcdef".into(),
            op: OP_UPSERT.into(),
            transaction_time: "2026-01-01T00:00:00Z".into(),
            device_id: "dev-hand".into(),
            content: SyncContent::Plain(serde_json::Value::Null),
            content_hash: clio_store::sha256_hex(
                crate::keyed_roles::published_digest_salt(),
                "anything",
            ),
            admission_receipt: None,
        },
        &key,
        &payload,
    );
    if let SyncContent::Cipher(cipher) = &mut mutation.content {
        cipher.alg = "aes256gcm".to_owned();
    }
    hand_push(&addr, mutation);
    pull(&cfg_b, &["bs".to_owned()]).expect("pull");
    let open = b
        .dead_letter_list("bs", DeadLetterStatus::Open, 10)
        .expect("dead letters");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].reason_code, "malformed");
    assert!(
        !open[0].detail.contains("task-"),
        "the refusal leaks no identifier: {}",
        open[0].detail
    );
    // The refusal names the held key, not the row's own algorithm: an
    // unimplemented algorithm fails every key this node holds, which is the
    // refusal path the pre-existing rotation tests already pin.
    assert!(
        open[0].detail.contains(&key.key_id()),
        "the refusal names the held key it tried: {}",
        open[0].detail
    );
    assert!(
        b.get_task("bs", TASK_ID).expect("get").is_none(),
        "nothing was applied"
    );
    server.stop();
}
