// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the request-side lifecycle of one peer's
//! denial action: the snapshot of the enabled peers as durable pending rows,
//! the start/finish helpers over the durable per-peer action store, and the
//! one fold that derives the record's peer state slots from the durable rows.
//!
//! ## Owns
//! - `peer_action_id`, the stable idempotency key of one (request, peer,
//!   denial) action
//! - `snapshot_peers`, `begin_denial`, `mark_delivered`, `mark_acknowledged`,
//!   `mark_refused`, and `record_disposition`, the durable lifecycle helpers
//! - `derived_peer_states`, the peer state slot fold
//!
//! ## Does not own
//! - The durable per-peer action row contract and its SQL (`clio-store`'s
//!   `peer_actions`)
//! - The frozen peer vocabulary and the record field
//!   (`subject_request_frozen`)
//! - Which peers a deployment enables and the request opened for them
//!   (`subject_request` owns register)
//! - The refusal, retention, and completion gate over the folded slots: this
//!   module records facts and derives the slots, and decides no outcome
//! - Delivering the operation to a peer (the sync transport owns that)
//!
//! ## Boundary
//!
//! Changes to how a peer action is keyed, settled, or folded into the record
//! should normally require changes here. Changes to the row contract or to the
//! completion gate should not require changes here.
//!
//! Keep this module focused on the per-peer request lifecycle.

use crate::subject_request::{PeerState, SubjectRequestRecord};
use crate::subject_request_events::ProtectedAction;
use clio_store::{
    PeerActionBinding, PeerActionDisposition, PeerActionKind, PeerActionRow, PeerActionState,
    PeerActionStore, PeerActionTarget,
};
use clio_types::{AmError, ErrorCode};
use std::collections::BTreeMap;

/// The stable idempotency key of one peer's denial under a request.
///
/// It extends the crate's erasure action id — the erasure producer id and the
/// request reference — with the peer, so the key is derived from the request
/// reference and the peer in the same producer-prefixed scheme every other
/// durable action id uses, never a second key scheme. The same derivation
/// binds every begin, advance, and read of the row, and survives a restart
/// because it carries no clock and no random value.
#[must_use]
pub fn peer_action_id(request_id: &str, peer_id: &str) -> String {
    format!(
        "{}:{request_id}:{peer_id}",
        ProtectedAction::Erasure.producer_id()
    )
}

/// Durably snapshot one pending denial row per enabled peer of `record`.
///
/// The record's frozen peer set is the deployment's enabled set: a
/// not-applicable record has no peers and snapshots nothing, while a required
/// record writes one `pending` row per peer under the request id and the
/// record's frozen subject/bank scope. The snapshot is written before any
/// erasure effect, and it is idempotent per peer: a repeated snapshot returns
/// the stored rows untouched. A failed snapshot may have written the earlier
/// peers; a retry returns those rows and writes only the missing ones.
///
/// # Errors
/// A blank required field, a row bound to a different action id, or a storage
/// failure. The record itself is expected to be persisted first, so a restart
/// replays the same snapshot from it.
pub fn snapshot_peers(
    store: &dyn PeerActionStore,
    record: &SubjectRequestRecord,
) -> Result<Vec<PeerActionRow>, AmError> {
    record
        .peers
        .keys()
        .map(|peer_id| begin_denial(store, &record.request_id, peer_id))
        .collect()
}

/// Begin one peer's denial idempotently and return the durable row.
///
/// The first call inserts `pending` / no disposition / no proof; a repeated
/// call returns the stored row untouched, so a crash-and-retry can neither
/// write a second row nor regress a settled one. A stored row bound to a
/// different action id is refused, because the idempotency key is fixed at the
/// first begin.
///
/// # Errors
/// A blank required field, a stored row bound to a different action id, or a
/// storage failure.
pub fn begin_denial(
    store: &dyn PeerActionStore,
    request_id: &str,
    peer_id: &str,
) -> Result<PeerActionRow, AmError> {
    let action_id = peer_action_id(request_id, peer_id);
    store.begin(&bound(request_id, peer_id, &action_id))
}

/// Record that one peer's denial operation was delivered.
///
/// Delivery is not acknowledgement: the row keeps no disposition and no proof
/// until the peer's durable proof arrives, so the fold keeps the peer pending.
///
/// # Errors
/// A missing row, an illegal transition, or a storage failure.
pub fn mark_delivered(
    store: &dyn PeerActionStore,
    request_id: &str,
    peer_id: &str,
) -> Result<PeerActionRow, AmError> {
    settle(
        store,
        request_id,
        peer_id,
        PeerActionTarget {
            to: PeerActionState::Delivered,
            disposition: PeerActionDisposition::Unknown,
            proof_ref: None,
        },
    )
}

/// Record a peer's durable acknowledgement of the denial.
///
/// The acknowledgement carries the peer's own durable proof — a recorded
/// generation, a subject hash, a salt fingerprint — because a transport
/// receipt or a read-back is not proof. The row keeps `removal_evidence` as
/// its disposition. A duplicate acknowledgement is a tolerated no-op: the
/// stored row, including its first proof, stays exactly as it is.
///
/// # Errors
/// A missing row, an illegal transition, or a storage failure. A blank proof
/// reference is refused.
pub fn mark_acknowledged(
    store: &dyn PeerActionStore,
    request_id: &str,
    peer_id: &str,
    proof_ref: &str,
) -> Result<PeerActionRow, AmError> {
    settle(
        store,
        request_id,
        peer_id,
        PeerActionTarget {
            to: PeerActionState::Acknowledged,
            disposition: PeerActionDisposition::RemovalEvidence,
            proof_ref: Some(proof_ref),
        },
    )
}

/// Record a peer's refusal of its denial operation, with no disposition.
///
/// A refusal alone proves nothing about the copy: the disposition stays
/// undecided, so the fold keeps the peer pending and no completion claim can
/// rest on it. A duplicate refusal is a tolerated no-op; a row already settled
/// with a disposition keeps it.
///
/// # Errors
/// A missing row, an illegal transition, or a storage failure.
pub fn mark_refused(
    store: &dyn PeerActionStore,
    request_id: &str,
    peer_id: &str,
) -> Result<PeerActionRow, AmError> {
    settle(
        store,
        request_id,
        peer_id,
        PeerActionTarget {
            to: PeerActionState::Refused,
            disposition: PeerActionDisposition::Unknown,
            proof_ref: None,
        },
    )
}

/// Record one peer's copy disposition with its durable proof.
///
/// A removed peer is settled `refused` with the recorded disposition — the
/// peer's deletion evidence or the approved retention — because the store
/// keeps a settled row fixed: the disposition can only travel with the settle
/// that moves the row, and a disposition recorded afterwards would be dropped.
/// The call tolerates a row that already holds exactly this disposition and
/// proof, and refuses a row settled with different evidence, so a caller never
/// believes a disposition was recorded when the durable row says otherwise.
///
/// # Errors
/// A missing row, an illegal transition, a row already settled with different
/// evidence ([`ErrorCode::Forbidden`]), or a storage failure.
pub fn record_disposition(
    store: &dyn PeerActionStore,
    request_id: &str,
    peer_id: &str,
    disposition: PeerActionDisposition,
    proof_ref: Option<&str>,
) -> Result<PeerActionRow, AmError> {
    let stored = settle(
        store,
        request_id,
        peer_id,
        PeerActionTarget {
            to: PeerActionState::Refused,
            disposition,
            proof_ref,
        },
    )?;
    if stored.disposition == disposition && stored.proof_ref.as_deref() == proof_ref {
        Ok(stored)
    } else {
        Err(AmError::new(
            ErrorCode::Forbidden,
            format!(
                "peer action for request `{request_id}` peer `{peer_id}` is settled with different evidence"
            ),
        ))
    }
}

/// The record's peer state slots, derived from the durable rows in one place.
///
/// A slot is [`PeerState::Acknowledged`] only when the peer's durable row is
/// acknowledged, [`PeerState::Disposed`] when the row records a disposition,
/// and [`PeerState::Pending`] otherwise. Only a row bound to the peer's own
/// derived action id counts: a row under a different key, for another peer,
/// for another request, or of another action kind never moves a slot, so a
/// restart re-derives exactly the frozen set from the durable rows and an
/// offline peer stays pending.
#[must_use]
pub fn derived_peer_states(
    record: &SubjectRequestRecord,
    rows: &[PeerActionRow],
) -> BTreeMap<String, PeerState> {
    record
        .peers
        .keys()
        .map(|peer_id| (peer_id.clone(), slot_state(record, peer_id, rows)))
        .collect()
}

/// The binding of one peer's denial row, under its derived action id.
fn bound<'a>(request_id: &'a str, peer_id: &'a str, action_id: &'a str) -> PeerActionBinding<'a> {
    PeerActionBinding {
        request_id,
        peer_id,
        action_kind: PeerActionKind::Denial,
        action_id,
    }
}

/// Advance one peer's denial row through its derived binding.
fn settle(
    store: &dyn PeerActionStore,
    request_id: &str,
    peer_id: &str,
    target: PeerActionTarget<'_>,
) -> Result<PeerActionRow, AmError> {
    let action_id = peer_action_id(request_id, peer_id);
    store.advance(&bound(request_id, peer_id, &action_id), &target)
}

/// The slot one peer's durable rows imply.
fn slot_state(record: &SubjectRequestRecord, peer_id: &str, rows: &[PeerActionRow]) -> PeerState {
    let expected = peer_action_id(&record.request_id, peer_id);
    let matched = rows.iter().find(|row| {
        row.request_id == record.request_id
            && row.peer_id == peer_id
            && row.action_kind == PeerActionKind::Denial
            && row.action_id == expected
    });
    match matched {
        Some(row) if row.state == PeerActionState::Acknowledged => PeerState::Acknowledged,
        Some(row) if row.disposition != PeerActionDisposition::Unknown => PeerState::Disposed,
        _ => PeerState::Pending,
    }
}

#[cfg(test)]
#[path = "subject_peer_tests.rs"]
mod subject_peer_tests;
