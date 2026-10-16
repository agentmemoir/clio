// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the durable per-peer request action
//! contract: one row per (request, peer, action kind), written before the
//! peer effect and kept pending until the peer's durable proof arrives, with
//! guarded transitions bound to the row key and its idempotency id.
//!
//! ## Owns
//! - `PeerActionStore`, its row/binding/target types, and the closed kind,
//!   state, and disposition domains with their fail-closed decoding
//! - The validation helpers, the transition table, and the shared column
//!   list the two driver halves in `peer_actions_backends` build their SQL on
//!
//! ## Does not own
//! - The per-backend SQL (`peer_actions_backends` owns the two driver halves)
//! - The `peer_actions` DDL text (`sql/003_local_compliance_slots.sql`,
//!   loaded through `migrate`)
//! - Which peers are enabled, the request record they hang off, or the sync
//!   transport that delivers one operation (callers own those)
//! - Folding a disposition into a request outcome: rows are stored verbatim
//!
//! ## Boundary
//!
//! Changes to which columns the store writes or reads, or to the transition
//! guard, should normally require changes here. Changes to how a request
//! folds its peer results should not require changes here.
//!
//! Keep this module focused on the durable per-peer action contract.

use clio_types::{AmError, ErrorCode};

/// The closed set of per-peer action kinds the store persists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerActionKind {
    /// The idempotent denial operation: the peer must make the subject's
    /// copies unreadable and confirm it durably.
    Denial,
}

impl PeerActionKind {
    /// The stored CHECK-domain spelling.
    fn as_db(self) -> &'static str {
        match self {
            Self::Denial => "denial",
        }
    }

    /// Read the stored spelling back, failing closed on anything else.
    fn from_db(kind: &str) -> Result<Self, AmError> {
        match kind {
            "denial" => Ok(Self::Denial),
            other => Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("peer_actions row has unknown action_kind `{other}`"),
            )),
        }
    }
}

/// The state of one peer action row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerActionState {
    /// The intent is durable and the operation has no delivery record yet.
    Pending,
    /// The operation was delivered; the peer's durable proof is still absent.
    Delivered,
    /// The peer confirmed a durable denial and the required purge state.
    Acknowledged,
    /// The peer refused, or retention kept the copies.
    Refused,
}

impl PeerActionState {
    /// The stored CHECK-domain spelling.
    fn as_db(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Acknowledged => "acknowledged",
            Self::Refused => "refused",
        }
    }

    /// Read the stored spelling back, failing closed on anything else.
    fn from_db(state: &str) -> Result<Self, AmError> {
        match state {
            "pending" => Ok(Self::Pending),
            "delivered" => Ok(Self::Delivered),
            "acknowledged" => Ok(Self::Acknowledged),
            "refused" => Ok(Self::Refused),
            other => Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("peer_actions row has unknown state `{other}`"),
            )),
        }
    }
}

/// The disposition of the peer's copy, separate from the action state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerActionDisposition {
    /// The peer proved the copy was removed or made unreadable.
    RemovalEvidence,
    /// The peer proved the copy is retained under an approved basis.
    RetentionApproved,
    /// No disposition is recorded yet.
    Unknown,
}

impl PeerActionDisposition {
    /// The stored CHECK-domain spelling.
    fn as_db(self) -> &'static str {
        match self {
            Self::RemovalEvidence => "removal_evidence",
            Self::RetentionApproved => "retention_approved",
            Self::Unknown => "unknown",
        }
    }

    /// Read the stored spelling back, failing closed on anything else.
    fn from_db(disposition: &str) -> Result<Self, AmError> {
        match disposition {
            "removal_evidence" => Ok(Self::RemovalEvidence),
            "retention_approved" => Ok(Self::RetentionApproved),
            "unknown" => Ok(Self::Unknown),
            other => Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("peer_actions row has unknown disposition `{other}`"),
            )),
        }
    }
}

/// One durable per-peer action row, as begun and read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerActionRow {
    /// The durable request the action belongs to.
    pub request_id: String,
    /// The peer that must act.
    pub peer_id: String,
    /// The action kind, fixed at first begin.
    pub action_kind: PeerActionKind,
    /// The stable idempotency key for this (request, peer, kind).
    pub action_id: String,
    /// The current action state.
    pub state: PeerActionState,
    /// The peer's copy disposition, separate from the state.
    pub disposition: PeerActionDisposition,
    /// The peer's durable proof for the recorded state, when it has one.
    pub proof_ref: Option<String>,
}

/// The row key and idempotency id a guarded transition must match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerActionBinding<'a> {
    /// The durable request the action belongs to.
    pub request_id: &'a str,
    /// The peer that must act.
    pub peer_id: &'a str,
    /// The action kind, fixed at first begin.
    pub action_kind: PeerActionKind,
    /// The stable idempotency key for this (request, peer, kind).
    pub action_id: &'a str,
}

/// The target of one guarded state advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerActionTarget<'a> {
    /// The state the advance moves to; the stored state itself is a no-op.
    pub to: PeerActionState,
    /// The disposition the advance records with the state.
    pub disposition: PeerActionDisposition,
    /// The peer's durable proof for the recorded state, when it has one.
    pub proof_ref: Option<&'a str>,
}

/// Persistence contract for durable per-peer request actions.
///
/// Implementations MUST durably record the intent ([`PeerActionStore::begin`])
/// before the peer effect runs, and MUST persist `pending` until the peer's
/// durable proof arrives: a transport receipt or a read-back is not proof.
/// A duplicate begin returns the stored row untouched, and a duplicate
/// acknowledgement is a tolerated no-op, so a retry cannot repeat a completed
/// effect. A restart reads rows back verbatim: nothing is auto-completed.
pub trait PeerActionStore: Send + Sync {
    /// Durably record the intent for one (request, peer, kind).
    ///
    /// The first begin inserts `pending` / `unknown` / no proof. A repeated
    /// begin — including one racing a first begin — returns the stored row
    /// untouched: no second row and no state regression. A repeated begin
    /// whose `action_id` differs from the stored one is refused, because the
    /// idempotency key is fixed at the first begin.
    ///
    /// # Errors
    /// Blank required fields, a stored row bound to a different `action_id`,
    /// or storage failures.
    fn begin(&self, binding: &PeerActionBinding<'_>) -> Result<PeerActionRow, AmError>;

    /// Move one row through the legal transition table.
    ///
    /// The advance must match `request_id`, `peer_id`, `action_kind`, and
    /// `action_id`; a mismatched binding is refused and changes nothing. The
    /// legal moves are `pending` to `delivered`, `acknowledged`, or `refused`,
    /// and `delivered` to `acknowledged` or `refused`; `acknowledged` and
    /// `refused` are terminal. A target equal to the stored state is a
    /// tolerated no-op returning the stored row, so a duplicate delivery or a
    /// duplicate acknowledgement cannot repeat a completed effect.
    ///
    /// # Errors
    /// Blank required fields, no stored row for the row key,
    /// [`ErrorCode::InvalidArgument`] for a different `action_id` or an
    /// illegal transition, or storage failures.
    fn advance(
        &self,
        binding: &PeerActionBinding<'_>,
        target: &PeerActionTarget<'_>,
    ) -> Result<PeerActionRow, AmError>;

    /// The stored row for one (request, peer, kind), if any.
    ///
    /// # Errors
    /// Storage failures, or [`ErrorCode::ConfigCorrupt`] when a stored enum
    /// spelling is not in the closed domain.
    fn load(
        &self,
        request_id: &str,
        peer_id: &str,
        action_kind: PeerActionKind,
    ) -> Result<Option<PeerActionRow>, AmError>;

    /// Every stored row for `request_id` — the frozen peer snapshot read —
    /// ordered by peer id, then action kind.
    ///
    /// # Errors
    /// Storage failures, or [`ErrorCode::ConfigCorrupt`] when a stored enum
    /// spelling is not in the closed domain.
    fn load_for_request(&self, request_id: &str) -> Result<Vec<PeerActionRow>, AmError>;
}

/// Refuse a binding that is missing a required field.
fn validate_binding(binding: &PeerActionBinding<'_>) -> Result<(), AmError> {
    for (name, value) in [
        ("request_id", binding.request_id),
        ("peer_id", binding.peer_id),
        ("action_id", binding.action_id),
    ] {
        if value.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("peer action {name} is required"),
            ));
        }
    }
    Ok(())
}

/// Refuse a target whose proof reference is present but blank.
fn validate_target(target: &PeerActionTarget<'_>) -> Result<(), AmError> {
    if target
        .proof_ref
        .is_some_and(|proof| proof.trim().is_empty())
    {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "peer action proof reference cannot be blank",
        ));
    }
    Ok(())
}

/// Whether `to` is a legal forward move out of `from`; the same state is a
/// tolerated no-op handled before this table is consulted.
fn transition_legal(from: PeerActionState, to: PeerActionState) -> bool {
    matches!(
        (from, to),
        (
            PeerActionState::Pending,
            PeerActionState::Delivered | PeerActionState::Acknowledged | PeerActionState::Refused
        ) | (
            PeerActionState::Delivered,
            PeerActionState::Acknowledged | PeerActionState::Refused
        )
    )
}

/// Refuse an advance whose idempotency key differs from the stored row's.
fn binding_conflict(binding: &PeerActionBinding<'_>) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        format!(
            "peer action for request `{}` peer `{}` is bound to a different action id",
            binding.request_id, binding.peer_id
        ),
    )
}

/// Refuse an advance with no stored row for the row key.
fn unknown_action(binding: &PeerActionBinding<'_>) -> AmError {
    AmError::new(
        ErrorCode::NotFound,
        format!(
            "no peer action row exists for request `{}` peer `{}`",
            binding.request_id, binding.peer_id
        ),
    )
}

/// Refuse a move the transition table does not allow.
fn illegal_transition(from: PeerActionState, to: PeerActionState) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        format!(
            "peer action in state `{}` cannot advance to `{}`",
            from.as_db(),
            to.as_db()
        ),
    )
}

/// The row an insert must have left behind; losing it is a storage defect.
fn lost_row() -> AmError {
    AmError::new(ErrorCode::Internal, "peer action insert left no row behind")
}

/// The row columns both driver halves read, in order.
pub(crate) const COLUMNS: &str =
    "request_id, peer_id, action_kind, action_id, state, disposition, proof_ref";

/// One stored row before the enum spellings are decoded.
pub(crate) struct StoredRow {
    /// The durable request the action belongs to.
    pub(crate) request_id: String,
    /// The peer that must act.
    pub(crate) peer_id: String,
    /// The raw action kind spelling.
    pub(crate) action_kind: String,
    /// The stable idempotency key.
    pub(crate) action_id: String,
    /// The raw state spelling.
    pub(crate) state: String,
    /// The raw disposition spelling.
    pub(crate) disposition: String,
    /// The peer's durable proof, when stored.
    pub(crate) proof_ref: Option<String>,
}

impl StoredRow {
    /// Decode the stored spellings, failing closed on anything unknown.
    pub(crate) fn decode(self) -> Result<PeerActionRow, AmError> {
        Ok(PeerActionRow {
            request_id: self.request_id,
            peer_id: self.peer_id,
            action_kind: PeerActionKind::from_db(&self.action_kind)?,
            action_id: self.action_id,
            state: PeerActionState::from_db(&self.state)?,
            disposition: PeerActionDisposition::from_db(&self.disposition)?,
            proof_ref: self.proof_ref,
        })
    }
}

/// The two driver halves of [`PeerActionStore`]: the SQLite and Postgres
/// begin, advance, and restart reads.
#[path = "peer_actions_backends.rs"]
mod peer_actions_backends;

#[cfg(test)]
#[path = "peer_actions_tests.rs"]
mod peer_actions_tests;

#[cfg(test)]
#[path = "peer_actions_pg_tests.rs"]
mod peer_actions_pg_tests;
