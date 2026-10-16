// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the frozen field vocabulary of the durable
//! subject-request record: the record itself and the slot types its fields
//! hold across a restart.
//!
//! ## Owns
//! - `SubjectRequestRecord`, the durable record a store persists
//! - `RequestScope`, the immutable bank/subject scope a request id binds to
//! - `RequestKind`, fixed when the request is opened
//! - `VerificationResult` and the operator deadline and exception fields
//! - `RequestAction`, the required copy, notice, and delivery action slots
//! - `PathSlot`, `PathState`, and `PathOwner`, the per-path state slots
//! - `PeerState`, the state slot of one peer from the frozen snapshot
//! - `PeerEnablement`, the peers-disabled/required fact recorded at open
//!
//! ## Does not own
//! - The request state table, commands, transitions, and the ledger
//!   register/apply contracts (`subject_request.rs`)
//! - Any storage adapter, SQL, CLI, or the delivery of a peer or copy action
//! - Subject content, identity proof, key material, or tokens: no field here
//!   holds them, and free-text reasons stay generic
//!
//! ## Boundary
//!
//! Changes to what a request persists, or to the frozen slot states, should
//! normally require changes here. Changes to how the request state moves
//! should not require changes here.
//!
//! Keep this module focused on the durable record schema.

use crate::subject_request::RequestState;
use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a request applies, fixed when the request is opened.
///
/// The scope of a stored request never changes; a reused id bound to a
/// different scope or request kind is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestScope {
    /// The whole bank: every subject the bank holds.
    Bank {
        /// The bank the request covers.
        bank_id: String,
    },
    /// One subject inside a bank.
    Subject {
        /// The bank the subject lives in.
        bank_id: String,
        /// The subject the request covers.
        subject_id: String,
    },
}

impl RequestScope {
    /// The contract scope check, shared with the action-event validator so no
    /// event carries a scope the contract would refuse.
    pub(crate) fn validate(&self) -> Result<(), AmError> {
        match self {
            Self::Bank { bank_id } => Self::validate_bank(bank_id),
            Self::Subject {
                bank_id,
                subject_id,
            } => {
                Self::validate_bank(bank_id)?;
                if subject_id.trim().is_empty() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "subject scope needs a subject id",
                    ));
                }
                Ok(())
            }
        }
    }

    fn validate_bank(bank_id: &str) -> Result<(), AmError> {
        if bank_id.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "request scope needs a bank id",
            ));
        }
        Ok(())
    }
}

/// The kind of work a request covers, fixed when the request is opened.
///
/// A replayed id with a changed kind is refused, like a changed scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// A copy of the subject's data is produced for the subject.
    Copy,
    /// The subject's data is verified-erased.
    Erasure,
}

/// The operator's identity-verification decision for the requester.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationResult {
    /// The operator verified the requester's identity.
    Verified,
    /// The operator could not verify the requester's identity.
    Unverified,
}

/// The kind of operator action a request can require.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestActionKind {
    /// Produce or hand over a copy.
    Copy,
    /// Give the subject notice of the result.
    Notice,
    /// Deliver the result to the covered peers.
    Delivery,
}

/// The open/closed state of one required action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestActionState {
    /// The action is still owed.
    Open,
    /// The action is done or the operator explicitly disposed of it.
    Closed,
}

/// One required operator action, keyed by a stable action id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestAction {
    /// What the action does.
    pub kind: RequestActionKind,
    /// Whether the action is still owed.
    pub state: RequestActionState,
}

/// Who controls a covered path.
///
/// Clio must complete its own paths; an external path, such as an operator or
/// recipient copy, stays open until the operator disposes of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathOwner {
    /// A product-controlled path Clio must complete.
    Clio,
    /// A path Clio does not control and must not claim to delete.
    External,
}

/// The state of one covered path.
///
/// Slots are set by explicit recording only: a restart reloads every path
/// exactly as stored and auto-completes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathState {
    /// No verified result yet.
    Pending,
    /// An attempt was saved and its result is pending reconciliation. The
    /// attempt and action ids name the saved attempt, so a restart resumes
    /// the reconciliation instead of repeating the effect.
    Saved {
        /// The saved attempt id.
        attempt_id: String,
        /// The saved action id.
        action_id: String,
    },
    /// The path has a verified result.
    Done,
    /// The path failed and needs the operator.
    Failed,
}

/// One covered path: who owns it and its current state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSlot {
    /// Who controls the path.
    pub owner: PathOwner,
    /// The current state of the path.
    pub state: PathState,
}

/// The state slot of one peer from the snapshot taken when the request was
/// opened.
///
/// The peer set is frozen at open; only the slots move. An offline peer
/// stays [`PeerState::Pending`]; nothing expires a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerState {
    /// The peer has not acknowledged yet.
    Pending,
    /// The peer acknowledged the erasure.
    Acknowledged,
    /// The operator explicitly disposed of the peer instead.
    Disposed,
}

/// Whether the request requires the frozen peer set, recorded once when the
/// request opens.
///
/// A peers-disabled or peer-absent deployment records
/// [`PeerEnablement::NotApplicable`]: a known fact of the deployment, never an
/// unknown value and never an acknowledgement. An enabled deployment records
/// [`PeerEnablement::Required`] together with the frozen per-peer slots. An
/// unrecognized stored spelling fails closed when the record decodes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerEnablement {
    /// No enabled peer records exist: the request has no peer work.
    #[default]
    NotApplicable,
    /// The request requires the frozen enabled peer set.
    Required,
}

impl PeerEnablement {
    /// The fact an enabled-peer snapshot implies: no labels means peer work
    /// is not applicable, a non-empty set means the peers are required.
    #[must_use]
    pub fn for_peer_set(peers: &BTreeMap<String, PeerState>) -> Self {
        if peers.is_empty() {
            Self::NotApplicable
        } else {
            Self::Required
        }
    }
}

/// One durable subject request: what it is, what it covers, and every slot
/// its completion depends on.
///
/// The whole record serializes to one canonical form, so a store can persist
/// it as a single value and compare it by equality after a restart. The id,
/// kind, and scope are fixed when the request is opened and never change.
/// Technical completion, operator action, and the final outcome are separate
/// results carried by [`RequestState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectRequestRecord {
    /// The durable request id.
    pub request_id: String,
    /// The kind of work, fixed when the request is opened.
    pub kind: RequestKind,
    /// The scope the id is bound to; never changes.
    pub scope: RequestScope,
    /// The current request state; its `Final` variant is the final outcome.
    pub state: RequestState,
    /// The operator's identity-verification decision, once recorded.
    pub verification: Option<VerificationResult>,
    /// The operator-supplied deadline, once recorded. Clio reports it as
    /// given and decides no legal applicability.
    pub deadline: Option<String>,
    /// The required copy, notice, and delivery actions, by action id.
    pub actions: BTreeMap<String, RequestAction>,
    /// The per-path state slots, by path id.
    pub paths: BTreeMap<String, PathSlot>,
    /// The peer snapshot captured when the request was opened, one state
    /// slot per peer. The peer set is frozen; only the slots move.
    pub peers: BTreeMap<String, PeerState>,
    /// Whether the request requires the frozen peer set: an enabled
    /// deployment records [`PeerEnablement::Required`], a peers-disabled or
    /// peer-absent one [`PeerEnablement::NotApplicable`]. A stored row from
    /// before this fact carries no value and loads as not applicable.
    #[serde(default)]
    pub peer_enablement: PeerEnablement,
    /// The approved retention exception, once recorded; generic text.
    pub exception: Option<String>,
}

#[cfg(test)]
#[path = "subject_request_frozen_tests.rs"]
mod subject_request_frozen_tests;
