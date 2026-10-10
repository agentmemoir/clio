// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading a record's real identifier back out
//! of its own decrypted payload, so the apply matrix and the conflict journal
//! see a real identifier rather than an opaque handle. It is the only place in
//! the workspace that resolves a handle, and it resolves it as a read of the
//! sealed payload rather than as an inversion of the handle.
//!
//! ## Owns
//! - Which payload field carries each handled kind's real identifier
//! - Replacing a wire handle with the identifier its payload actually holds
//! - The refusal reason for a payload that carries no identifier
//!
//! ## Does not own
//! - The handle derivation and the kinds it covers (`keyed_roles`)
//! - Opening the ciphertext, which happens before this runs (`client`)
//! - Dead-letter persistence (`apply_dead_letter`)
//!
//! ## Boundary
//!
//! Changes to where a handled kind's real identifier lives inside its payload
//! should normally require changes here. Changes to the apply matrix should not
//! require changes here, because they never see a handle.
//!
//! Keep this module focused on handle resolution.

use crate::protocol::{
    ENTITY_KIND_ASSOC, ENTITY_KIND_FAILURE, ENTITY_KIND_PERSONA, ENTITY_KIND_TASK,
};
use serde_json::Value;

/// Why a row's wire identifier could not be resolved to a real identifier.
#[derive(Debug)]
pub(crate) enum ResolveFailure {
    /// The payload carries no identifier for this kind, so the wire value stays
    /// an opaque string with nothing to validate it against.
    MissingIdentifier,
}

impl ResolveFailure {
    /// The operator-facing refusal. It names the kind and the bank so the row
    /// can be found, and names neither the wire value nor anything from a
    /// handle's preimage, because the dead-letter table is read by operators
    /// and must not become a place a handle lands.
    pub(crate) fn reason(&self, kind: &str, bank: &str) -> String {
        match self {
            Self::MissingIdentifier => format!(
                "encrypted {kind} row in bank {bank} carries an opaque identifier handle, but its \
                 decrypted payload holds no matching real identifier, so the record cannot be \
                 applied; discard the row deliberately with `clio sync ack-skip` or re-push it \
                 from the node that wrote it"
            ),
        }
    }
}

/// Put the record's real identifier back on the mutation, replacing an opaque
/// wire handle.
///
/// Returns the mutation unchanged when the row is not one this workspace
/// handles: a plaintext row carries its real identifier already, and the kinds
/// whose identifiers are machine-generated never had a handle. A handled row
/// whose payload lacks its identifier is a refusal, never a partially resolved
/// mutation, because an opaque string left in the store's business-identifier
/// column would key the conflict journal and the apply guard on it.
pub(crate) fn resolve_entity_id(
    m: &mut crate::protocol::SyncMutation,
    plain: &Value,
) -> Result<(), ResolveFailure> {
    if !crate::keyed_roles::kind_uses_handle(&m.entity_kind) {
        return Ok(());
    }
    let Some(real_id) = real_id_from_payload(&m.entity_kind, plain) else {
        return Err(ResolveFailure::MissingIdentifier);
    };
    m.entity_id = real_id;
    Ok(())
}

/// The record's real identifier as it sits inside its sealed payload.
///
/// Each handled kind already seals the whole record, so this is a read of a
/// field the payload has always carried and not an added wire field.
fn real_id_from_payload(kind: &str, plain: &Value) -> Option<String> {
    let raw = match kind {
        ENTITY_KIND_PERSONA => plain.get("entry")?.get("key")?,
        ENTITY_KIND_TASK => plain.get("task")?.get("task_id")?,
        ENTITY_KIND_FAILURE => plain.get("failure")?.get("id")?,
        ENTITY_KIND_ASSOC => plain.get("id")?,
        _ => return None,
    };
    raw.as_str().map(str::to_owned)
}

#[cfg(test)]
#[path = "handle_resolution_tests.rs"]
mod tests;
