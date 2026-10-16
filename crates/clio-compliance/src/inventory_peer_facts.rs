// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for what one ledger run's peer inputs let the
//! compliance scope ledger say about the enabled-peers family: the explicit
//! enablement fact, the durable per-peer acknowledgement states, and the
//! source, gap, and open reasons that render them.
//!
//! ## Owns
//! - `PeerFact`, the peer view of one ledger run's inputs
//! - The enabled-peers source text, its superseding gap, and its open reasons
//!
//! ## Does not own
//! - The ledger row schema and every other family (`inventory`)
//! - The durable per-peer action row contract (`clio-store`'s `peer_actions`)
//! - Which peers a deployment enables, and reading their durable rows: the
//!   caller supplies both the enablement fact and the rows it read back
//! - The evidence rule itself (`subject_peer`'s `peer_row_settled`): this
//!   module reads it and never re-derives it
//! - Whether a peer's result settles a request (the request record owns that)
//!
//! ## Boundary
//!
//! Changes to how the peer fact is reported should normally require changes
//! here. Changes to the row schema or to the durable row contract should not.
//!
//! Keep this module focused on rendering the peer fact of one ledger run.

use super::{InventoryInput, RowPlan, RowSource};
use crate::subject_peer::{peer_action_id, peer_row_settled};
use clio_store::{PeerActionDisposition, PeerActionKind, PeerActionRow, PeerActionState};

/// The reason a peer fact with no enablement fact stays open.
const UNVERIFIED: &str = "peers could not be verified: no enablement fact was supplied";
/// The reason a bound row's recorded removal evidence stays open.
const UNPROVEN_REMOVAL: &str = "pending, removal evidence is unproven";
/// The reason an enabled deployment without supplied rows stays open.
const ENABLED_NO_RECORD: &str =
    "peers are enabled and no acknowledgement record exists to verify them";

/// What one ledger run's inputs let the ledger say about the peer family.
pub(super) enum PeerFact<'a> {
    /// No enablement fact was supplied: the peer fact stays unverified.
    Unverified,
    /// Configuration proves the deployment has no enabled peers.
    NotApplicable,
    /// Peers are enabled; the caller supplied these durable rows, possibly none.
    Enabled(&'a [PeerActionRow]),
}

impl<'a> PeerFact<'a> {
    /// Read the peer fact from one ledger run's inputs.
    ///
    /// A peers-disabled deployment with no durable rows is the known
    /// not-applicable mode. A disabled deployment that still holds durable
    /// rows reports those rows instead: a stored record outranks the
    /// configuration fact, so a stale row is never hidden.
    pub(super) fn of(input: &'a InventoryInput) -> Self {
        match input.peers_enabled {
            None => Self::Unverified,
            Some(false) if input.peer_actions.is_empty() => Self::NotApplicable,
            Some(_) => Self::Enabled(&input.peer_actions),
        }
    }

    /// The named gap the row carries; every other row keeps its plan gap.
    pub(super) fn superseding_gap(&self, plan: &RowPlan) -> Option<String> {
        if !is_peers(plan) {
            return plan.gap.map(str::to_owned);
        }
        match self {
            Self::Unverified | Self::Enabled([]) => plan.gap.map(str::to_owned),
            Self::NotApplicable | Self::Enabled(_) => None,
        }
    }

    /// The named open reasons the peer fact adds to the peers row.
    pub(super) fn reasons(&self, plan: &RowPlan) -> Vec<String> {
        if !is_peers(plan) {
            return Vec::new();
        }
        match self {
            Self::Unverified => vec![UNVERIFIED.to_owned()],
            Self::NotApplicable => Vec::new(),
            Self::Enabled([]) => vec![ENABLED_NO_RECORD.to_owned()],
            Self::Enabled(rows) => rows.iter().filter_map(unsettled_reason).collect(),
        }
    }

    /// The resolved source description of the peers row.
    pub(super) fn source(&self) -> String {
        match self {
            Self::Unverified => "peer enablement and acknowledgements (not stored)".to_owned(),
            Self::NotApplicable => "peer enablement: not applicable (no enabled peers)".to_owned(),
            Self::Enabled([]) => {
                "peer enablement: enabled (no acknowledgement record supplied)".to_owned()
            }
            Self::Enabled(rows) => format!(
                "durable peer action records (peer_actions): {}",
                rows.iter()
                    .map(|row| format!("{} {}", row.peer_id, state_phrase(row)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// Whether `plan` is the enabled-peers family row.
fn is_peers(plan: &RowPlan) -> bool {
    matches!(plan.source, RowSource::Peers)
}

/// Whether one durable row is bound to the peer's own derived action id: the
/// same binding the request fold applies, checked per row because a ledger run
/// holds no request record. A row under another key, another request, or
/// another peer never settles anything.
fn is_bound(row: &PeerActionRow) -> bool {
    row.action_kind == PeerActionKind::Denial
        && row.action_id == peer_action_id(&row.request_id, &row.peer_id)
}

/// One durable row's state in the request fold's words: a bound acknowledged
/// row is settled, a bound removal the row carries the evidence for is a
/// disposed row, anything else is pending. A removal evidence without its
/// proof reference is not evidence, so it is named pending too. An unbound row
/// is named as unbound and is never rendered as acknowledged.
fn state_phrase(row: &PeerActionRow) -> &'static str {
    if !is_bound(row) {
        return "unbound durable row";
    }
    if row.state == PeerActionState::Acknowledged {
        return "acknowledged";
    }
    match row.disposition {
        // The crate's one evidence rule decides the removal arm, so a removal
        // the durable row cannot show is never reported disposed.
        PeerActionDisposition::RemovalEvidence if peer_row_settled(row) => {
            "disposed (removal evidence)"
        }
        PeerActionDisposition::RemovalEvidence => UNPROVEN_REMOVAL,
        PeerActionDisposition::RetentionApproved => "disposed (retention approved)",
        PeerActionDisposition::Unknown if row.state == PeerActionState::Refused => {
            "refused, no disposition recorded"
        }
        PeerActionDisposition::Unknown => "pending acknowledgement",
    }
}

/// The named reason one durable row is not settled: `None` for a bound row
/// whose disposition is an approved retention — a disposition this surface
/// reports as recorded, because the request record owns whether a retention
/// settles the request — and for a bound row the crate's one evidence rule
/// (`peer_row_settled`) settles. Every other bound row is named open — a
/// removal evidence without its proof reference is not evidence — and a row
/// that fails the binding rule is always unsettled and always named.
fn unsettled_reason(row: &PeerActionRow) -> Option<String> {
    if !is_bound(row) {
        return Some(format!(
            "peer `{}`: durable row is not bound to the derived action id",
            row.peer_id
        ));
    }
    if row.disposition == PeerActionDisposition::RetentionApproved || peer_row_settled(row) {
        return None;
    }
    let phrase = if row.disposition == PeerActionDisposition::RemovalEvidence {
        UNPROVEN_REMOVAL
    } else if row.state == PeerActionState::Refused {
        "refused, no disposition recorded"
    } else {
        "pending acknowledgement"
    };
    Some(format!("peer `{}`: {phrase}", row.peer_id))
}
