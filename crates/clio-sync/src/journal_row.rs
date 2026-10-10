// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for building the durable sync journal row from
//! a mutation envelope: content sealing under the local node's DEKs (server
//! role) or verbatim client ciphertext, plus the subject-id extraction used
//! for journal sealing.
//!
//! ## Owns
//! - `SyncJournalRow` construction from a wire mutation
//!
//! ## Does not own
//! - Apply semantics or the trust matrix (see `apply`)
//! - Persistence SQL (`clio-store` `SyncStore`)
//!
//! ## Boundary
//!
//! Changes to journal row shape should normally require changes here and in
//! `clio-store`'s sync persistence together.
//!
//! Keep this module focused on journal row construction.

use crate::apply::ApplyEngine;
use crate::clock::now_utc;
use crate::protocol::{SyncContent, SyncMutation};
use clio_store::SyncJournalRow;
use clio_types::AmError;

pub fn journal_row(engine: &ApplyEngine<'_>, m: &SyncMutation) -> Result<SyncJournalRow, AmError> {
    let content_sealed = match (&m.content, engine.kms) {
        (SyncContent::Plain(v), Some(kms)) => {
            let subject_id = item_subject_id(v);
            let bytes = serde_json::to_vec(v).expect("serialize journal content");
            Some(
                serde_json::to_string(&kms.encrypt(&subject_id, &bytes)?)
                    .expect("serialize envelope"),
            )
        }
        (SyncContent::Plain(_), None) => None,
        (SyncContent::Cipher(_), _) => {
            Some(serde_json::to_string(m).expect("serialize cipher journal row"))
        }
    };
    Ok(SyncJournalRow {
        seq: 0,
        sync_mutation_id: m.sync_mutation_id.clone(),
        bank_id: m.bank.clone(),
        entity_kind: m.entity_kind.clone(),
        entity_id: m.entity_id.clone(),
        op: m.op.clone(),
        transaction_time: m.transaction_time.clone(),
        device_id: m.device_id.clone(),
        content_hash: m.content_hash.clone(),
        content_sealed,
        client_encrypted: matches!(m.content, SyncContent::Cipher(_)),
        admission_receipt: m
            .admission_receipt
            .as_ref()
            .map(serde_json::Value::to_string),
        created_at: now_utc(),
    })
}

pub fn item_subject_id(content: &serde_json::Value) -> String {
    content
        .get("subject_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("sync-journal")
        .to_owned()
}
