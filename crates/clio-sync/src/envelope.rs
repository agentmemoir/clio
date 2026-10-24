// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the canonical, versioned encoding of one
//! sync mutation's execution envelope, the byte string the AEAD authenticates
//! alongside the sealed content.
//!
//! ## Owns
//! - The associated-data layout: label, version byte, then the encoded envelope
//! - The envelope field list and its fixed order
//! - `ExecutionView`, the borrowed view of those fields for one mutation
//!
//! ## Does not own
//! - Sealing and opening (`crypto`), which take the AAD as an argument
//! - Wire envelope shapes (`protocol`)
//! - Which fields apply reads from the outer envelope (`apply`)
//!
//! ## Boundary
//!
//! Changes to which envelope fields the ciphertext is bound to should normally
//! require changes here. Changes to the AEAD choice or key handling should not.
//!
//! Keep this module focused on the envelope's canonical encoding.
//!
//! # Layout
//!
//! `AAD_LABEL`, one version byte, then the JSON encoding of the envelope
//! struct. The struct's field order is fixed and `serde_json` writes struct
//! fields in declaration order, so the same envelope always produces the same
//! bytes, and no field is joined to a bare string that another field could
//! absorb.

use crate::protocol::SyncMutation;
use serde::Serialize;

/// Domain label that starts every associated-data byte string.
pub(crate) const AAD_LABEL: &[u8] = b"am-sync-v1";

/// Envelope encoding version; a reader refuses an encoding it does not know by
/// failing to authenticate rather than by opening under a different layout.
const ENVELOPE_VERSION: u8 = 1;

/// The envelope fields, in the order they are encoded. Every field is bound:
/// a relay that retargets any of them breaks authentication.
#[derive(Serialize)]
struct Envelope<'a> {
    v: u8,
    sync_mutation_id: &'a str,
    bank: &'a str,
    entity_kind: &'a str,
    entity_id: &'a str,
    op: &'a str,
    transaction_time: &'a str,
    device_id: &'a str,
    content_hash: &'a str,
    /// The receipt exactly as it travels on the wire: present only when no
    /// cipher key is configured, and absent when it rides inside the sealed
    /// document instead.
    admission_receipt: Option<&'a serde_json::Value>,
}

/// One mutation's execution envelope, borrowed. `content` is deliberately
/// excluded: it is what the AEAD seals, and it is not known before the seal.
pub(crate) struct ExecutionView<'a> {
    sync_mutation_id: &'a str,
    bank: &'a str,
    entity_kind: &'a str,
    entity_id: &'a str,
    op: &'a str,
    transaction_time: &'a str,
    device_id: &'a str,
    content_hash: &'a str,
    admission_receipt: Option<&'a serde_json::Value>,
}

impl ExecutionView<'_> {
    /// The associated data for this envelope.
    ///
    /// # Panics
    /// Serializing this struct cannot fail; it holds no map and no value that
    /// `serde_json` rejects.
    pub(crate) fn aad(&self) -> Vec<u8> {
        let envelope = Envelope {
            v: ENVELOPE_VERSION,
            sync_mutation_id: self.sync_mutation_id,
            bank: self.bank,
            entity_kind: self.entity_kind,
            entity_id: self.entity_id,
            op: self.op,
            transaction_time: self.transaction_time,
            device_id: self.device_id,
            content_hash: self.content_hash,
            admission_receipt: self.admission_receipt,
        };
        let mut aad = Vec::with_capacity(AAD_LABEL.len() + 1);
        aad.extend_from_slice(AAD_LABEL);
        aad.push(ENVELOPE_VERSION);
        aad.extend(serde_json::to_vec(&envelope).expect("serialize sync envelope"));
        aad
    }
}

impl SyncMutation {
    /// The execution envelope of this mutation as it currently stands.
    pub(crate) fn execution_view(&self) -> ExecutionView<'_> {
        ExecutionView {
            sync_mutation_id: &self.sync_mutation_id,
            bank: &self.bank,
            entity_kind: &self.entity_kind,
            entity_id: &self.entity_id,
            op: &self.op,
            transaction_time: &self.transaction_time,
            device_id: &self.device_id,
            content_hash: &self.content_hash,
            admission_receipt: self.admission_receipt.as_ref(),
        }
    }
}

#[cfg(test)]
#[path = "envelope_tests.rs"]
mod tests;
