// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for per-entity mutation packaging inside the
//! push change feed: turning one local row (memory item, association edge,
//! closed triple, persona stable entry, persona preference, belief, task
//! version, failure record) into a wire `SyncMutation` with a stable
//! `sync_mutation_id` and the node's optional sync cipher applied.
//!
//! ## Owns
//! - The item / assoc / triple / persona / belief / task / failure packaging
//!   functions and their shared helpers (`content_for`, `content_and_receipt`,
//!   `wire_entity_id`, `mutation_id`, `content_hash`, `am_store_now`)
//! - Which identifier each mutation puts on the wire: the real one, or the
//!   opaque handle for a kind whose identifier carries caller-chosen text
//! - Where the origin admission receipt travels: inside the sealed document
//!   with a cipher configured, and beside the document on the wire without one
//!
//! ## Does not own
//! - Feed ordering, watermark cursors, and push ack handling (`client_feed`)
//! - Wire envelope shapes (`protocol`)
//! - The digest and handle derivations themselves (`keyed_roles`)
//! - Resolving a handle back to the real identifier on the pull path (`client`)
//! - Apply-side semantics for these mutations (`apply`, `apply_records`)
//!
//! ## Boundary
//!
//! Changes to how one row becomes one wire mutation should normally require
//! changes here. Changes to feed paging or cursor advancement should not.
//!
//! Keep this module focused on push-side mutation packaging.

use crate::client::SyncClientConfig;
use crate::protocol::{
    ENTITY_KIND_ASSOC, ENTITY_KIND_BELIEF, ENTITY_KIND_FAILURE, ENTITY_KIND_ITEM,
    ENTITY_KIND_PERSONA, ENTITY_KIND_TASK, ENTITY_KIND_TRIPLE, OP_INVALIDATE, OP_UPSERT,
    SyncContent, SyncMutation,
};
use clio_store::AssocEdgeRecord;
use clio_types::{
    AmError, BeliefObject, FailureRecord, MemoryItem, PersonaPreference, PersonaStableEntry,
    TaskRecord,
};
use serde_json::Value;

/// Package one memory item as an item mutation (`upsert` or the feed row's
/// non-upsert op such as `discard`).
pub(crate) fn item_mutation(
    cfg: &SyncClientConfig,
    item: &MemoryItem,
    op: &str,
) -> Result<SyncMutation, AmError> {
    let plain: Value = match op {
        OP_UPSERT => serde_json::to_value(item).expect("serialize item"),
        _ => serde_json::json!({}),
    };
    let content_hash = content_hash(cfg, &plain);
    let receipt = item
        .admission_score
        .map(|score| serde_json::json!({ "admitted": true, "score": score }));
    let identity_stamp = match op {
        OP_UPSERT => item.updated_at.clone(),
        _ => am_store_now(item),
    };
    let m = SyncMutation {
        sync_mutation_id: mutation_id(cfg, &item.id, &identity_stamp, op),
        bank: item.bank_id.clone(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        // An item id is machine-generated, so the relay learns nothing from it.
        entity_id: item.id.clone(),
        op: op.to_owned(),
        transaction_time: identity_stamp.clone(),
        device_id: cfg.device_id.clone(),
        content: SyncContent::Plain(Value::Null),
        content_hash,
        admission_receipt: wire_receipt(cfg, receipt.as_ref()),
    };
    finish_mutation(cfg, m, &plain, receipt)
}

pub(crate) fn assoc_mutation(
    cfg: &SyncClientConfig,
    bank: &str,
    edge: &AssocEdgeRecord,
) -> Result<SyncMutation, AmError> {
    let plain = serde_json::to_value(edge).expect("serialize edge");
    let content_hash = content_hash(cfg, &plain);
    let m = SyncMutation {
        // The edge id embeds the caller's relationship label, so the wire
        // identifier is a handle while the idempotency key stays on the real
        // id. The real id is inside the sealed edge record.
        sync_mutation_id: mutation_id(cfg, &edge.id, &edge.last_updated_at, "assoc"),
        bank: bank.to_owned(),
        entity_kind: ENTITY_KIND_ASSOC.into(),
        entity_id: wire_entity_id(cfg, ENTITY_KIND_ASSOC, bank, &edge.id),
        op: OP_UPSERT.into(),
        transaction_time: edge.last_updated_at.clone(),
        device_id: cfg.device_id.clone(),
        content: SyncContent::Plain(Value::Null),
        content_hash,
        admission_receipt: wire_receipt(cfg, None),
    };
    finish_mutation(cfg, m, &plain, None)
}

/// Package one closed triple as an `invalidate` mutation. Open triples (not yet
/// closed on either time axis) are not outbound changes.
pub(crate) fn triple_mutation(
    cfg: &SyncClientConfig,
    bank: &str,
    entity_id: &str,
) -> Result<Option<SyncMutation>, AmError> {
    let Some(triple) = cfg.store.get_triple(bank, entity_id)? else {
        return Ok(None);
    };
    let Some(close) = triple
        .tx_until
        .clone()
        .or_else(|| triple.valid_until.clone())
    else {
        return Ok(None);
    };
    let plain = serde_json::json!({
        "subject": triple.subject,
        "predicate": triple.predicate,
        "object": triple.object,
    });
    let content_hash = content_hash(cfg, &plain);
    let m = SyncMutation {
        sync_mutation_id: mutation_id(cfg, &triple.id, &close, OP_INVALIDATE),
        bank: bank.to_owned(),
        entity_kind: ENTITY_KIND_TRIPLE.into(),
        // A triple id is machine-generated, so it travels as it is.
        entity_id: triple.id.clone(),
        op: OP_INVALIDATE.into(),
        transaction_time: close,
        device_id: cfg.device_id.clone(),
        content: SyncContent::Plain(Value::Null),
        content_hash,
        admission_receipt: wire_receipt(cfg, None),
    };
    finish_mutation(cfg, m, &plain, None).map(Some)
}

/// Package one task version as a `task` upsert: the carrier item plus the
/// structured record. The real identifier is the task id and the idempotency
/// key is `(task_id, record id, upsert)`, so every version is distinct and
/// replays stably. The wire `entity_id` is that same task id in plaintext mode
/// and an opaque handle of it when a cipher is configured.
pub(crate) fn task_mutation(
    cfg: &SyncClientConfig,
    bank: &str,
    stamp: &str,
    task: &TaskRecord,
    item: &MemoryItem,
) -> Result<SyncMutation, AmError> {
    let plain = serde_json::json!({ "item": item, "task": task });
    let content_hash = content_hash(cfg, &plain);
    let receipt = item
        .admission_score
        .map(|score| serde_json::json!({ "admitted": true, "score": score }));
    let m = SyncMutation {
        // The task id is the caller-supplied string this phase exists to hide,
        // so it leaves as a handle; the idempotency key still names it, under a
        // key the relay does not hold.
        sync_mutation_id: mutation_id(cfg, &task.task_id, &task.id, OP_UPSERT),
        bank: bank.to_owned(),
        entity_kind: ENTITY_KIND_TASK.into(),
        entity_id: wire_entity_id(cfg, ENTITY_KIND_TASK, bank, &task.task_id),
        op: OP_UPSERT.into(),
        transaction_time: stamp.to_owned(),
        device_id: cfg.device_id.clone(),
        content: SyncContent::Plain(Value::Null),
        content_hash,
        admission_receipt: wire_receipt(cfg, receipt.as_ref()),
    };
    finish_mutation(cfg, m, &plain, receipt)
}

/// Package one failure record as a `failure` upsert (carrier item + record).
pub(crate) fn failure_mutation(
    cfg: &SyncClientConfig,
    bank: &str,
    stamp: &str,
    failure: &FailureRecord,
    item: &MemoryItem,
) -> Result<SyncMutation, AmError> {
    let plain = serde_json::json!({ "item": item, "failure": failure });
    let content_hash = content_hash(cfg, &plain);
    let receipt = item
        .admission_score
        .map(|score| serde_json::json!({ "admitted": true, "score": score }));
    let m = SyncMutation {
        // A failure id embeds the caller's task id, so it leaves as a handle.
        sync_mutation_id: mutation_id(cfg, &failure.id, stamp, OP_UPSERT),
        bank: bank.to_owned(),
        entity_kind: ENTITY_KIND_FAILURE.into(),
        entity_id: wire_entity_id(cfg, ENTITY_KIND_FAILURE, bank, &failure.id),
        op: OP_UPSERT.into(),
        transaction_time: stamp.to_owned(),
        device_id: cfg.device_id.clone(),
        content: SyncContent::Plain(Value::Null),
        content_hash,
        admission_receipt: wire_receipt(cfg, receipt.as_ref()),
    };
    finish_mutation(cfg, m, &plain, receipt)
}

/// Package one persona stable-entry version as a `persona` upsert. The wire
/// payload carries the opened plaintext entry (values/contexts re-seal at
/// rest on each node); `entity_id` is the attr key and the mutation stamp is
/// the row's change watermark.
pub(crate) fn persona_stable_mutation(
    cfg: &SyncClientConfig,
    bank: &str,
    stamp: &str,
    entry: &PersonaStableEntry,
) -> Result<SyncMutation, AmError> {
    let plain = serde_json::json!({
        "record": "stable",
        "entry": entry,
    });
    record_mutation(
        cfg,
        bank,
        stamp,
        ENTITY_KIND_PERSONA,
        &plain,
        Some(entry.admission_score),
    )
}

/// Package one persona preference row as a `persona` upsert.
pub(crate) fn persona_pref_mutation(
    cfg: &SyncClientConfig,
    bank: &str,
    stamp: &str,
    pref: &PersonaPreference,
) -> Result<SyncMutation, AmError> {
    let plain = serde_json::json!({
        "record": "preference",
        "entry": pref,
    });
    record_mutation(
        cfg,
        bank,
        stamp,
        ENTITY_KIND_PERSONA,
        &plain,
        Some(pref.admission_score),
    )
}

/// Package one belief object (full trajectory) as a `belief` upsert.
pub(crate) fn belief_mutation(
    cfg: &SyncClientConfig,
    bank: &str,
    stamp: &str,
    belief: &BeliefObject,
) -> Result<SyncMutation, AmError> {
    let plain = serde_json::to_value(belief).expect("serialize belief");
    record_mutation(cfg, bank, stamp, ENTITY_KIND_BELIEF, &plain, None)
}

/// Shared packaging for domain-record mutations: plaintext logical content,
/// stable idempotency key on `(entity, change stamp, op)`, bank-scoped. The
/// real identifier is the persona attr key (payload `entry.key`) or the belief
/// id (payload `id`); the wire `entity_id` is a handle of it for the persona
/// kinds and the identifier itself for `belief`, whose ids are machine
/// generated.
fn record_mutation(
    cfg: &SyncClientConfig,
    bank: &str,
    stamp: &str,
    kind: &str,
    plain: &Value,
    admission_score: Option<f64>,
) -> Result<SyncMutation, AmError> {
    let real_id = match kind {
        ENTITY_KIND_PERSONA => plain
            .get("entry")
            .and_then(|e| e.get("key"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        // A belief id is machine-generated; this arm is what keeps it
        // unchanged on the wire.
        _ => plain
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    };
    let content_hash = content_hash(cfg, plain);
    let receipt =
        admission_score.map(|score| serde_json::json!({ "admitted": true, "score": score }));
    let m = SyncMutation {
        // The idempotency key always names the real identifier, including for
        // the persona kinds whose wire identifier is a handle.
        sync_mutation_id: mutation_id(cfg, &real_id, stamp, OP_UPSERT),
        bank: bank.to_owned(),
        entity_kind: kind.to_owned(),
        entity_id: wire_entity_id(cfg, kind, bank, &real_id),
        op: OP_UPSERT.into(),
        transaction_time: stamp.to_owned(),
        device_id: cfg.device_id.clone(),
        content: SyncContent::Plain(Value::Null),
        content_hash,
        admission_receipt: wire_receipt(cfg, receipt.as_ref()),
    };
    finish_mutation(cfg, m, plain, receipt)
}

fn am_store_now(item: &MemoryItem) -> String {
    match (&item.tx_until, &item.valid_until) {
        (Some(tx_until), _) => tx_until.clone(),
        (None, Some(valid_until)) => valid_until.clone(),
        (None, None) => item.updated_at.clone(),
    }
}

fn content_for(
    cfg: &SyncClientConfig,
    content: &Value,
    aad: &[u8],
) -> Result<SyncContent, AmError> {
    // The newest key is the first entry. An empty list means encryption off,
    // which is what every existing deployment has today, so content goes out
    // as `Plain` and `seal` is never called.
    let Some(key) = cfg.cipher.first() else {
        return Ok(SyncContent::Plain(content.clone()));
    };
    // Serializing a `serde_json::Value` cannot fail.
    let bytes = serde_json::to_vec(content).expect("serialize sync content");
    Ok(SyncContent::Cipher(key.seal(&bytes, aad)?))
}

/// Key of the origin admission receipt inside a sealed content document.
///
/// The receipt rides inside the ciphertext when a cipher is configured, so a
/// relay holding the journal never reads the score. The name is namespaced so
/// it cannot collide with a field of the record sharing the document.
pub(crate) const ADMISSION_RECEIPT_KEY: &str = "clio_admission_receipt";

/// Package one record for the wire: the content to send and the receipt that
/// travels beside it.
///
/// With a cipher configured the receipt is written into the sealed document
/// under `ADMISSION_RECEIPT_KEY` and the wire receipt is `None`, so the relay
/// learns no score. Without one the document is the record alone and the
/// receipt stays on the wire, byte for byte what a plaintext deployment has
/// always sent. The content digest is computed by the caller over the record
/// and not over this envelope, so it is unchanged in both modes.
fn content_and_receipt(
    cfg: &SyncClientConfig,
    plain: &Value,
    receipt: Option<Value>,
    aad: &[u8],
) -> Result<(SyncContent, Option<Value>), AmError> {
    let wire = wire_receipt(cfg, receipt.as_ref());
    if cfg.cipher.is_empty() {
        return Ok((content_for(cfg, plain, aad)?, wire));
    }
    let document = match receipt {
        Some(receipt) => {
            let mut document = plain.clone();
            document[ADMISSION_RECEIPT_KEY] = receipt;
            document
        }
        None => plain.clone(),
    };
    Ok((content_for(cfg, &document, aad)?, wire))
}

/// The receipt the wire copy carries: present only without a cipher, since
/// with one it rides inside the sealed document instead.
fn wire_receipt(cfg: &SyncClientConfig, receipt: Option<&Value>) -> Option<Value> {
    if cfg.cipher.is_empty() {
        receipt.cloned()
    } else {
        None
    }
}

/// Seal the content for `m` under its own execution envelope, which is final
/// in every other field by the time this runs, so the content is set last.
fn finish_mutation(
    cfg: &SyncClientConfig,
    mut m: SyncMutation,
    plain: &Value,
    receipt: Option<Value>,
) -> Result<SyncMutation, AmError> {
    let (content, wire) = content_and_receipt(cfg, plain, receipt, &m.execution_view().aad())?;
    debug_assert_eq!(
        wire, m.admission_receipt,
        "the wire receipt decision must match the mutation's"
    );
    m.content = content;
    Ok(m)
}

/// The wire identifier for one record: the opaque handle when a cipher is
/// configured and the kind carries caller-chosen text, and the real identifier
/// otherwise.
///
/// `real_id` is what the mutation id is derived from in every case, and it
/// stays inside the sealed payload either way, so the receiving side can read
/// it back after opening the content.
fn wire_entity_id(cfg: &SyncClientConfig, kind: &str, bank: &str, real_id: &str) -> String {
    if !crate::keyed_roles::kind_uses_handle(kind) {
        return real_id.to_owned();
    }
    crate::keyed_roles::identifier_handle(cfg.cipher.first(), kind, bank, real_id)
}

/// The digest a relay sees for this record's content: keyed under the digest
/// role's own subkey when a cipher is configured, and the plain unkeyed hash
/// otherwise, byte for byte what a plaintext deployment has always stored.
fn content_hash(cfg: &SyncClientConfig, content: &Value) -> String {
    let text = serde_json::to_string(content).unwrap_or_default();
    crate::keyed_roles::content_digest(cfg.cipher.first(), &text)
}

/// Stable idempotency key per logical mutation event.
///
/// Always over the **real** identifier, never over the wire handle: a mutation
/// id is what makes redelivery a duplicate, and two distinct records that
/// happened to share a handle must not share one of these. With a cipher the
/// key itself is derived under the mutation-id role's subkey, so a relay cannot
/// hash a guessed identifier and confirm it against the stored key; without one
/// it is the plain hash this has always been.
fn mutation_id(cfg: &SyncClientConfig, real_id: &str, stamp: &str, op: &str) -> String {
    crate::keyed_roles::mutation_id(cfg.cipher.first(), real_id, stamp, op)
}

#[cfg(test)]
#[path = "client_feed_package_digest_tests.rs"]
mod digest_tests;
#[cfg(test)]
#[path = "client_feed_package_handle_tests.rs"]
mod handle_tests;
#[cfg(test)]
#[path = "client_feed_package_receipt_tests.rs"]
mod receipt_tests;
#[cfg(test)]
#[path = "client_feed_package_tests.rs"]
mod tests;
