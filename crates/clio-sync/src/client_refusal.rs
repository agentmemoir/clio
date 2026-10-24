// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the operator-facing refusal reasons the sync
//! receive path records on a dead letter when it cannot trust a pulled row.
//!
//! ## Owns
//! - The sealed-mode refusal of plaintext content
//! - The missing-key and failed-open refusals of a sealed row
//!
//! ## Does not own
//! - When a row is refused, and what a refusal does to the cursor (`client`)
//! - Opening, resolving, or applying a pulled row (`client`, `apply`)
//!
//! ## Boundary
//!
//! Changes to what a refusal says should normally require changes here.
//! Changes to the receive policy itself should not require changes here.
//!
//! Keep this module focused on the refusal wording.

/// The refusal a plaintext row gets at a node configured with a sync key: the
/// node is sealed, the row carries no authenticated envelope, and a relay can
/// fabricate or downgrade a plaintext row freely, so it is quarantined rather
/// than applied. Stable wording the receive tests pin.
pub(crate) const PLAINTEXT_REFUSED_REASON: &str = "sealed mode refused plaintext content: this node holds a sync key, so it accepts only sealed \
     content, and a plaintext row carries no authenticated envelope. Discard the row deliberately \
     with the `sync_ack_skip` MCP tool, passing its mutation id with confirm";

/// The refusal for a row sealed under a key this node does not hold. It names
/// the stamped identifier and the identifiers held, both derived and
/// non-reversible, and the recovery. It never carries a key or a prefix of
/// one; the identifier is the operator's clue, the store-byte fingerprint is
/// what `clio sync key status` prints.
pub(crate) fn missing_key_reason(stamped: &str, held: &[String]) -> String {
    format!(
        "sync content is sealed under key {stamped}, which this node does not hold; held keys: \
         {}. Add the missing key on this node and pull again, or discard the row deliberately \
         with the `sync_ack_skip` MCP tool, passing its mutation id with confirm",
        held.join(", "),
    )
}

/// The refusal for a stamped row its own matching held key cannot open. A
/// failed open has three possible causes and the message names all of them in
/// the order an operator should weigh them: the row predates this release and
/// was sealed under the older associated-data layout, another key sealed it, or
/// the bytes are damaged or were altered. It names the deliberate discard.
pub(crate) fn open_failed_reason(stamped: &str) -> String {
    format!(
        "sync cipher stamped with key {stamped} could not be opened with the matching held key; \
         possible causes: a row sealed before this release used an older associated-data layout; \
         a different key sealed it; the ciphertext may be damaged or tampered with. Discard the \
         row deliberately with the `sync_ack_skip` MCP tool, passing its mutation id with confirm"
    )
}

#[cfg(test)]
#[path = "client_refusal_tests.rs"]
mod client_refusal_tests;
