// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests how one pulled wire mutation becomes the row apply sees: the
//! ack-skipped passthrough that returns the mutation unchanged, and the normal
//! path that opens a sealed row, resolves its identifier, and restores the
//! origin admission receipt.
//!
//! ## Owns
//! - Behavior coverage for `decrypt_for_apply` and the receipt restoration it
//!   performs
//!
//! ## Does not own
//! - The refusal wording a rejected row carries (`client_refusal_tests`)
//! - The pull loop and cursor policy (`client_tests`)
//! - Receipt packaging at the sender (`client_feed_package_receipt_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the opened-row shape changes; they never change it.
//!
//! Keep this module focused on the pulled-row tests.

use super::decrypt_for_apply;
use crate::apply::apply_tests::fixture;
use crate::client::client_tests::{cfg, paired_key};
use crate::client_feed::package::ADMISSION_RECEIPT_KEY;
use crate::envelope_fixtures::seal_content_for;
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation};
use clio_store::{DeadLetterStatus, SqliteStore, SyncStore};
use serde_json::json;
use std::sync::Arc;

/// The one fixed stamp these tests use; no test reads the clock.
const STAMP: &str = "2026-01-01T00:00:00Z";

/// One item mutation carrying `content` under the given ids.
fn item_mutation(mid: &str, entity: &str, content: serde_json::Value) -> SyncMutation {
    SyncMutation {
        sync_mutation_id: mid.into(),
        bank: "bp".into(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        entity_id: entity.into(),
        op: OP_UPSERT.into(),
        transaction_time: STAMP.into(),
        device_id: "dev-a".into(),
        content: SyncContent::Plain(content),
        content_hash: format!("h-{mid}"),
        admission_receipt: None,
    }
}

/// A config for this node holding the paired key against `store`.
fn node(store: &Arc<SqliteStore>) -> crate::client::SyncClientConfig {
    cfg("dev-b", store, "127.0.0.1:1", vec![paired_key()])
}

/// The sealed row a writer sends: its item document, plus the receipt when one
/// is given.
fn sealed(mid: &str, entity: &str, gist: &str, receipt: Option<serde_json::Value>) -> SyncMutation {
    let mut document = json!({
        "id": entity,
        "bank_id": "bp",
        "subject_id": "s1",
        "gist": gist,
        "updated_at": STAMP,
    });
    if let Some(receipt) = receipt {
        document[ADMISSION_RECEIPT_KEY] = receipt;
    }
    let mutation = item_mutation(mid, entity, document.clone());
    seal_content_for(mutation, &paired_key(), &document)
}

/// A row the operator already ack-skipped passes through unchanged, even on a
/// node that holds a key and would otherwise refuse its plaintext content.
#[test]
fn an_ack_skipped_row_passes_through_unchanged() {
    let (store, _kms) = fixture();
    let m = item_mutation("m-skip", "i-1", json!({ "id": "i-1", "gist": "plain" }));
    store
        .dead_letter_open(&clio_store::SyncDeadLetterRow {
            sync_mutation_id: m.sync_mutation_id.clone(),
            bank_id: m.bank.clone(),
            reason_code: "malformed".into(),
            detail: "refused once".into(),
            envelope_json: "{}".into(),
            status: DeadLetterStatus::Open,
            first_seen: STAMP.into(),
            last_error: None,
            updated_at: None,
        })
        .expect("open dead letter");
    assert!(
        store
            .dead_letter_ack_skip(&m.sync_mutation_id, STAMP)
            .expect("ack skip"),
        "the row is skipped"
    );

    let envelope = serde_json::to_string(&m).expect("envelope");
    let got = decrypt_for_apply(&node(&store), &m, &envelope)
        .expect("the pull runs")
        .expect("the row passes through");
    assert_eq!(got, m, "the skipped row is returned unchanged");
    assert_eq!(
        store
            .dead_letter_count("bp", DeadLetterStatus::Open)
            .expect("count"),
        0,
        "the passthrough adds no refusal"
    );
}

/// The normal path opens a sealed row: the content becomes the sealed payload
/// and the origin receipt is lifted back onto the mutation.
#[test]
fn a_sealed_row_opens_with_its_receipt_restored() {
    let (store, _kms) = fixture();
    let m = sealed(
        "m-open",
        "i-open",
        "opened",
        Some(json!({ "admitted": true, "score": 0.75 })),
    );
    let envelope = serde_json::to_string(&m).expect("envelope");
    let got = decrypt_for_apply(&node(&store), &m, &envelope)
        .expect("the pull runs")
        .expect("the row opens");
    assert_eq!(got.sync_mutation_id, "m-open");
    assert_eq!(got.entity_id, "i-open");
    let SyncContent::Plain(document) = &got.content else {
        panic!("the opened row carries plaintext content");
    };
    assert_eq!(document["gist"], json!("opened"));
    assert_eq!(
        got.admission_receipt,
        Some(json!({ "admitted": true, "score": 0.75 })),
        "the sealed receipt is restored onto the mutation"
    );
    assert_eq!(
        store
            .dead_letter_count("bp", DeadLetterStatus::Open)
            .expect("count"),
        0,
        "an opened row leaves no refusal"
    );
}

/// A sealed payload with no receipt leaves the wire receipt absent: the
/// documented absent-score fallback, not an error and not an empty object.
#[test]
fn a_sealed_row_without_a_receipt_restores_none() {
    let (store, _kms) = fixture();
    let m = sealed("m-no-receipt", "i-2", "bare", None);
    let envelope = serde_json::to_string(&m).expect("envelope");
    let got = decrypt_for_apply(&node(&store), &m, &envelope)
        .expect("the pull runs")
        .expect("the row opens");
    assert_eq!(got.admission_receipt, None);
}

/// A payload whose receipt carries no numeric score restores nothing, so a
/// malformed receipt degrades to the documented zero rather than travelling.
#[test]
fn a_receipt_without_a_numeric_score_is_not_restored() {
    let (store, _kms) = fixture();
    let m = sealed(
        "m-bad-receipt",
        "i-3",
        "odd",
        Some(json!({ "admitted": true, "score": "high" })),
    );
    let envelope = serde_json::to_string(&m).expect("envelope");
    let got = decrypt_for_apply(&node(&store), &m, &envelope)
        .expect("the pull runs")
        .expect("the row opens");
    assert_eq!(got.admission_receipt, None);
}
