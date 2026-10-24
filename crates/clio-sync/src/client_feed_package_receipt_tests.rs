// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing where a packaged mutation's origin
//! admission receipt travels: inside the sealed document with the wire field
//! absent when a cipher is configured, and on the wire unchanged when no
//! cipher is held.
//!
//! ## Owns
//! - Packaging-level assertions for the admission receipt
//!
//! ## Does not own
//! - The pull-side restoration (`client_receipt_tests`)
//! - The content digest (`client_feed_package_digest_tests`)
//! - The wire identifier (`client_feed_package_handle_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the receipt's wire or document position changes; they
//! never change it.
//!
//! Keep this module focused on the packaged admission receipt.

use crate::client_feed::package::{
    ADMISSION_RECEIPT_KEY, assoc_mutation, failure_mutation, item_mutation, persona_pref_mutation,
    persona_stable_mutation, task_mutation,
};
use crate::client_feed_package_fixtures::{
    BANK, FAILURE_ID, STAMP, TASK_ID, carrier, edge, failure_record, item, plain_cfg, preference,
    sealed_cfg, stable_entry, task_record,
};
use crate::protocol::{SyncContent, SyncMutation};
use serde_json::{Value, json};

/// The decrypted document a packaging call sealed, opened with the key its
/// fixture config holds.
fn opened_document(cfg: &crate::client::SyncClientConfig, mutation: &SyncMutation) -> Value {
    let SyncContent::Cipher(cipher) = &mutation.content else {
        panic!("a cipher-configured packaging call must seal");
    };
    let key = cfg.cipher.first().expect("a held key");
    let aad = mutation.execution_view().aad();
    let bytes = key.open(cipher, &aad).expect("open the sealed document");
    serde_json::from_slice(&bytes).expect("the sealed document is JSON")
}

/// With a cipher the item's receipt rides inside the sealed document and the
/// wire field is absent.
#[test]
fn a_sealed_item_carries_its_receipt_inside_the_document() {
    let cfg = sealed_cfg(&crate::credential_fixtures::secret("pkg-receipt-item"));
    let mutation = item_mutation(&cfg, &item("itm-1-2-3"), "upsert").expect("item");
    assert!(
        mutation.admission_receipt.is_none(),
        "the relay must see no receipt: {:?}",
        mutation.admission_receipt
    );
    let document = opened_document(&cfg, &mutation);
    assert_eq!(
        document[ADMISSION_RECEIPT_KEY],
        json!({ "admitted": true, "score": 0.5 }),
        "the score is sealed with the record"
    );
    assert_eq!(
        document["id"], "itm-1-2-3",
        "the record still rides the document"
    );
}

/// Without a cipher the receipt stays on the wire and the document is the
/// record alone.
#[test]
fn plaintext_mode_keeps_the_receipt_on_the_wire() {
    let cfg = plain_cfg();
    let mutation = item_mutation(&cfg, &item("itm-1-2-3"), "upsert").expect("item");
    assert_eq!(
        mutation.admission_receipt,
        Some(json!({ "admitted": true, "score": 0.5 }))
    );
    let SyncContent::Plain(document) = &mutation.content else {
        panic!("plaintext mode seals nothing");
    };
    assert!(
        document.get(ADMISSION_RECEIPT_KEY).is_none(),
        "no receipt marker joins the plaintext document: {document}"
    );
    assert_eq!(document["id"], "itm-1-2-3");
}

/// Every score-bearing kind seals its receipt and leaves the wire field absent.
#[test]
fn a_sealed_task_failure_and_persona_each_seal_their_receipt() {
    let cfg = sealed_cfg(&crate::credential_fixtures::secret("pkg-receipt-kinds"));
    let task = task_mutation(
        &cfg,
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("task");
    let failure = failure_mutation(
        &cfg,
        BANK,
        STAMP,
        &failure_record(FAILURE_ID, TASK_ID),
        &carrier("car-fail"),
    )
    .expect("failure");
    let stable = persona_stable_mutation(&cfg, BANK, STAMP, &stable_entry("synthetic_attr_name"))
        .expect("persona stable");
    let pref = persona_pref_mutation(&cfg, BANK, STAMP, &preference("synthetic_pref_name"))
        .expect("persona preference");
    for (label, mutation) in [
        ("task", task),
        ("failure", failure),
        ("persona stable", stable),
        ("persona preference", pref),
    ] {
        assert!(
            mutation.admission_receipt.is_none(),
            "{label}: the relay must see no receipt"
        );
        let document = opened_document(&cfg, &mutation);
        assert_eq!(
            document[ADMISSION_RECEIPT_KEY]["score"], 0.5,
            "{label}: the score is sealed"
        );
        assert_eq!(
            document[ADMISSION_RECEIPT_KEY]["admitted"], true,
            "{label}: the admitted flag is sealed"
        );
    }
}

/// An explicit edge has no admission score, so it carries no receipt on the
/// wire and no receipt key in its sealed document.
#[test]
fn a_sealed_edge_carries_no_receipt_anywhere() {
    let cfg = sealed_cfg(&crate::credential_fixtures::secret("pkg-receipt-edge"));
    let edge_id = clio_store::explicit_id(BANK, "i-1", "i-2", "rel");
    let mutation = assoc_mutation(&cfg, BANK, &edge(&edge_id, "rel")).expect("edge");
    assert!(mutation.admission_receipt.is_none());
    let document = opened_document(&cfg, &mutation);
    assert!(
        document.get(ADMISSION_RECEIPT_KEY).is_none(),
        "an edge has no score to seal: {document}"
    );
    assert_eq!(document["id"], edge_id.as_str());
}
