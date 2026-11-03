// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sealed on-disk form of one MemTree node
//! field, the summary gist, so no readable summary of a bank's memories sits in
//! the node table.
//!
//! ## Owns
//! - Sealing a node's `summary_gist` into a field-bound ciphertext envelope
//! - Opening a stored `summary_gist` back into in-memory plaintext
//!
//! ## Does not own
//! - The envelope shape and the DEK provider contract (see `dek`)
//! - MemTree SQL and row mapping (see `sqlite_memtree`, `postgres_memtree`)
//! - How an ancestor gist is derived from its children (see `clio-write`)
//!
//! ## Boundary
//!
//! Changes to what a sealed gist is bound to should normally require changes
//! here. Changes to the envelope shape should not require changes here.
//!
//! Keep this module focused on the summary-gist seal and open.

use crate::dek::{CiphertextEnvelope, DekProvider};
use crate::memtree_model::MemTreeNodeRecord;
use clio_types::AmError;

/// Field role of a sealed summary gist in the envelope AAD.
const GIST_FIELD: &str = "memtree.summary_gist";

/// Seal one node's `summary_gist` for storage under its bank's DEK.
///
/// # Errors
/// Missing or destroyed key, or AEAD failure.
pub(crate) fn seal_gist(
    kms: &dyn DekProvider,
    node: &MemTreeNodeRecord,
) -> Result<Option<String>, AmError> {
    match node.summary_gist.as_deref() {
        None => Ok(None),
        Some(gist) => Ok(Some(
            kms.encrypt(GIST_FIELD, &node.id, &node.bank_id, gist.as_bytes())?
                .to_storage_string(),
        )),
    }
}

/// Open a stored `summary_gist` back into plaintext.
///
/// `None` covers three states: no gist, an empty column, or an unreadable body
/// (for example a crypto-shredded bank). MemTree reads must keep working after
/// erasure, so decryption failures are not errors here — the same tolerance as
/// [`crate::persona_store::open_persona_context`].
pub(crate) fn open_gist(
    kms: &dyn DekProvider,
    bank_id: &str,
    node_id: &str,
    stored: Option<&str>,
) -> Option<String> {
    let stored = stored.filter(|s| !s.is_empty())?;
    let envelope = CiphertextEnvelope::from_storage_string(stored).ok()?;
    if envelope.subject_id != bank_id {
        return None;
    }
    let bytes = kms.decrypt(GIST_FIELD, node_id, &envelope).ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
#[path = "memtree_seal_tests.rs"]
mod memtree_seal_tests;
