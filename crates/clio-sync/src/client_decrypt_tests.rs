// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the pull-side open of one mutation:
//! that a sealed document opens and restores its admission receipt, that every
//! row this node cannot trust is quarantined under the row's bank, and that an
//! operator `ack_skip` passes through unopened.
//!
//! ## Owns
//! - The refusal branch of each unusable content shape, and its stated reason
//! - The admission receipt lifted back out of an opened payload
//!
//! ## Does not own
//! - The pull loop and cursor policy (`client_tests`, `client_freshness_tests`)
//! - The key material and cipher shape (`crypto`)
//! - Handle resolution (`client_resolve_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a pulled row stops being opened or refused the way the
//! operator was told; they never change it.
//!
//! Keep this module focused on opening a pulled row before apply.

use super::decrypt_for_apply;
use crate::client::client_tests::{cfg, item, sqlite};
use crate::client_refusal::{PLAINTEXT_REFUSED_KEYLESS_REASON, PLAINTEXT_REFUSED_REASON};
use crate::crypto::SyncCipherKey;
use crate::protocol::{SyncContent, SyncMutation};
use base64::Engine as _;
use clio_store::{DeadLetterStatus, SqliteStore, SyncDeadLetterRow, SyncStore as _};
use serde_json::{Value, json};
use std::sync::Arc;

/// One bank for every case here, and one fixed stamp: no case reads the clock.
const BANK: &str = "bd";
const STAMP: &str = "2026-01-01T00:00:00Z";

fn key(secret: &str) -> SyncCipherKey {
    SyncCipherKey::from_secret(&crate::credential_fixtures::secret(secret))
}

/// One item upsert as it travels on the wire, before any sealing.
fn row(mid: &str) -> SyncMutation {
    crate::apply_fixtures::env(
        mid,
        BANK,
        crate::protocol::ENTITY_KIND_ITEM,
        "i-1",
        crate::protocol::OP_UPSERT,
        STAMP,
        "dev-a",
        crate::apply_fixtures::item_value(BANK, "i-1", "s1", STAMP, "g"),
    )
}

/// The same row with `document` sealed under `key`.
fn sealed(key: &SyncCipherKey, mid: &str, document: &Value) -> SyncMutation {
    let mut m = row(mid);
    let aad = m.execution_view().aad();
    let bytes = serde_json::to_vec(document).expect("document");
    m.content = SyncContent::Cipher(key.seal(&bytes, &aad).expect("seal"));
    m
}

/// The detail this node recorded on the open dead-letter row of `mid`.
fn detail_of(store: &Arc<SqliteStore>, mid: &str) -> String {
    store
        .dead_letter_list(BANK, DeadLetterStatus::Open, 10)
        .expect("rows")
        .into_iter()
        .find(|row| row.sync_mutation_id == mid)
        .unwrap_or_else(|| panic!("{mid} has no open dead-letter row"))
        .detail
}

#[test]
fn an_opened_document_restores_the_receipt_before_apply() {
    let store = sqlite();
    let key = key("decrypt-receipt-key");
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", vec![key.clone()]);
    let record = item(BANK, "i-1", "s1", STAMP, "restored");
    let mut document = serde_json::to_value(&record).expect("record");
    document[crate::client_feed::package::ADMISSION_RECEIPT_KEY] =
        json!({ "admitted": true, "score": 0.9 });
    let mutation = sealed(&key, "m-receipt", &document);
    let envelope = serde_json::to_string(&mutation).expect("envelope");
    let opened = decrypt_for_apply(&cfg, &mutation, &envelope)
        .expect("decrypt")
        .expect("an item row resolves");
    assert_eq!(
        opened.admission_receipt,
        Some(json!({ "admitted": true, "score": 0.9 })),
        "the score is back on the mutation before apply"
    );
}

#[test]
fn an_absent_or_malformed_receipt_degrades_to_the_documented_fallback() {
    let store = sqlite();
    let key = key("decrypt-fallback-key");
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", vec![key.clone()]);
    let record = item(BANK, "i-1", "s1", STAMP, "fallback");
    let cases: [(&str, Value); 2] = [
        ("no receipt key", Value::Null),
        (
            "malformed score",
            json!({ "admitted": true, "score": "high" }),
        ),
    ];
    for (label, receipt) in cases {
        let mut document = serde_json::to_value(&record).expect("record");
        if !receipt.is_null() {
            document[crate::client_feed::package::ADMISSION_RECEIPT_KEY] = receipt;
        }
        let mutation = sealed(&key, "m-fallback", &document);
        let envelope = serde_json::to_string(&mutation).expect("envelope");
        let opened = decrypt_for_apply(&cfg, &mutation, &envelope)
            .expect("decrypt")
            .expect("the row resolves");
        assert!(
            opened.admission_receipt.is_none(),
            "{label}: the receipt stays absent, so the comparison applies 0.0"
        );
    }
}

#[test]
fn a_plaintext_row_is_refused_when_a_key_is_held() {
    let store = sqlite();
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", vec![key("plain-key")]);
    let m = row("m-plain");
    let envelope = serde_json::to_string(&m).expect("envelope");
    assert!(
        decrypt_for_apply(&cfg, &m, &envelope)
            .expect("decrypt")
            .is_none()
    );
    assert_eq!(detail_of(&store, "m-plain"), PLAINTEXT_REFUSED_REASON);
}

#[test]
fn a_plaintext_row_is_refused_when_no_key_is_held() {
    let store = sqlite();
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", Vec::new());
    let m = row("m-keyless");
    let envelope = serde_json::to_string(&m).expect("envelope");
    assert!(
        decrypt_for_apply(&cfg, &m, &envelope)
            .expect("decrypt")
            .is_none()
    );
    assert_eq!(
        detail_of(&store, "m-keyless"),
        PLAINTEXT_REFUSED_KEYLESS_REASON
    );
}

#[test]
fn a_cipher_row_is_refused_when_no_key_is_configured() {
    let store = sqlite();
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", Vec::new());
    let m = sealed(&key("unheld-key"), "m-nokey", &json!({ "id": "i-1" }));
    let envelope = serde_json::to_string(&m).expect("envelope");
    assert!(
        decrypt_for_apply(&cfg, &m, &envelope)
            .expect("decrypt")
            .is_none()
    );
    assert_eq!(
        detail_of(&store, "m-nokey"),
        "no sync cipher key configured"
    );
}

#[test]
fn a_cipher_row_without_a_key_identifier_is_refused() {
    let store = sqlite();
    let held = key("stamped-key");
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", vec![held.clone()]);
    let mut m = sealed(&held, "m-noid", &json!({ "id": "i-1" }));
    if let SyncContent::Cipher(cipher) = &mut m.content {
        cipher.key_id = None;
    }
    let envelope = serde_json::to_string(&m).expect("envelope");
    assert!(
        decrypt_for_apply(&cfg, &m, &envelope)
            .expect("decrypt")
            .is_none()
    );
    assert!(
        detail_of(&store, "m-noid").contains("no key identifier"),
        "the operator is told the row carries no identifier"
    );
}

#[test]
fn a_cipher_row_stamped_with_an_unknown_key_names_the_missing_key() {
    let store = sqlite();
    let stamped = key("stamped-only-key");
    let held = key("held-key");
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", vec![held.clone()]);
    let m = sealed(&stamped, "m-unknown", &json!({ "id": "i-1" }));
    let envelope = serde_json::to_string(&m).expect("envelope");
    assert!(
        decrypt_for_apply(&cfg, &m, &envelope)
            .expect("decrypt")
            .is_none()
    );
    let detail = detail_of(&store, "m-unknown");
    assert!(detail.contains(&stamped.key_id()), "{detail}");
    assert!(detail.contains(&held.key_id()), "{detail}");
}

#[test]
fn a_ciphertext_the_stamped_key_cannot_open_is_refused() {
    let store = sqlite();
    let held = key("tamper-key");
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", vec![held.clone()]);
    let mut m = sealed(&held, "m-tampered", &json!({ "id": "i-1" }));
    if let SyncContent::Cipher(cipher) = &mut m.content {
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(&cipher.ciphertext_b64)
            .expect("base64");
        bytes[0] ^= 1;
        cipher.ciphertext_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    }
    let envelope = serde_json::to_string(&m).expect("envelope");
    assert!(
        decrypt_for_apply(&cfg, &m, &envelope)
            .expect("decrypt")
            .is_none()
    );
    assert!(
        detail_of(&store, "m-tampered").contains(&held.key_id()),
        "the reason names the key that could not open the row"
    );
}

#[test]
fn a_ciphertext_that_opens_to_non_json_is_refused() {
    let store = sqlite();
    let held = key("raw-key");
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", vec![held.clone()]);
    let mut m = row("m-raw");
    let aad = m.execution_view().aad();
    m.content = SyncContent::Cipher(held.seal(b"not json", &aad).expect("seal"));
    let envelope = serde_json::to_string(&m).expect("envelope");
    assert!(
        decrypt_for_apply(&cfg, &m, &envelope)
            .expect("decrypt")
            .is_none()
    );
    assert_eq!(
        detail_of(&store, "m-raw"),
        "decrypted sync content was not JSON"
    );
}

#[test]
fn an_ack_skipped_row_passes_through_unopened() {
    let store = sqlite();
    let cfg = cfg("dev-a", &store, "127.0.0.1:1", Vec::new());
    let m = row("m-ack");
    store
        .dead_letter_open(&SyncDeadLetterRow {
            sync_mutation_id: "m-ack".into(),
            bank_id: BANK.into(),
            reason_code: "malformed".into(),
            detail: "seeded by the test".into(),
            envelope_json: "{}".into(),
            status: DeadLetterStatus::Open,
            first_seen: STAMP.into(),
            last_error: None,
            updated_at: None,
        })
        .expect("seed the row");
    assert!(
        store
            .dead_letter_ack_skip("m-ack", STAMP)
            .expect("ack skip")
    );
    let envelope = serde_json::to_string(&m).expect("envelope");
    let opened = decrypt_for_apply(&cfg, &m, &envelope)
        .expect("decrypt")
        .expect("an ack-skipped row passes through");
    assert_eq!(opened.sync_mutation_id, "m-ack");
}
