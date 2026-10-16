// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module folds one stored request record into the separated status
//! report: its technical-completion field, its operator outcome, its pending
//! operator actions from the record, its copy rows, and its durable pending
//! local-event actions, its derived peer slots, and the retry report that wraps
//! the fold.
//!
//! ## Owns
//! - `TechnicalCompletion`, `RequestOutcome`, `PendingOperatorAction`,
//!   `RequestStatusReport`, and `RetryReport`
//! - `report_of`, its pending-event fold, and its state-to-field helpers
//!
//! ## Does not own
//! - The request-verb sequencing over the stores (`subject_request_service.rs`)
//! - The record vocabulary and the record/row mapping
//!   (`subject_request_frozen.rs`, `subject_request_store_wiring.rs`)
//!
//! ## Boundary
//!
//! Changes to how a status report is folded from a record should normally
//! require changes here; changes to how the request verbs are sequenced should
//! not.
//!
//! Keep this module focused on folding one record into its report.

use crate::copy_disposition::CopyRequiredAction;
use crate::erasure_retry::request_id_of;
use crate::subject_peer::derived_peer_states;
use crate::subject_request::{
    FinalOutcome, PeerEnablement, PeerState, RequestActionKind, RequestActionState, RequestKind,
    RequestScope, RequestState, StateCategory, SubjectRequestRecord, VerificationResult,
};
use crate::subject_request_events::ProtectedAction;
use crate::subject_request_service::SubjectRequestEnv;
use crate::subject_request_store_wiring::copy_row_disposition;
use clio_types::AmError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Whether the technical work of a request is done, separate from its outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TechnicalCompletion {
    /// The technical work is still running or has not started.
    Pending,
    /// The technical work is done.
    Complete,
    /// An exception left the technical state unknown.
    Unknown,
}

/// The operator-facing outcome of a request, separate from its technical work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    /// The request is not final.
    Open,
    /// Every covered path is done.
    Completed,
    /// Some covered paths are done and some could not be.
    PartiallyCompleted,
    /// The request was denied.
    Refused,
    /// An exception stopped the request.
    Blocked,
}

/// One operator action the request still owes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingOperatorAction {
    /// The stable id of the owed action.
    pub action_id: String,
    /// The kind of action the request still owes, when a known kind names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<RequestActionKind>,
}

/// The separated status of one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RequestStatusReport {
    /// The durable request id.
    pub request_id: String,
    /// The kind of work the request covers.
    pub kind: RequestKind,
    /// The immutable scope the id is bound to.
    pub scope: RequestScope,
    /// The current request state.
    pub state: RequestState,
    /// Whether the technical work is done, separate from the outcome.
    pub technical_completion: TechnicalCompletion,
    /// The operator actions the request still owes.
    pub pending_operator_actions: Vec<PendingOperatorAction>,
    /// The operator-facing outcome, separate from the technical work.
    pub request_outcome: RequestOutcome,
    /// The peer state slots derived from the durable peer rows.
    pub peers: BTreeMap<String, PeerState>,
    /// Whether the request requires the frozen peer set.
    pub peer_enablement: PeerEnablement,
    /// The operator's identity-verification decision, once recorded.
    pub verification: Option<VerificationResult>,
    /// The operator-supplied deadline, once recorded.
    pub deadline: Option<String>,
    /// The approved retention exception, once recorded.
    pub exception: Option<String>,
}

/// What one erasure retry did, with the effect counts it moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryReport {
    /// The status report folded after the retry.
    pub report: RequestStatusReport,
    /// The erasure action id, named only while that action was pending.
    pub action_id: Option<String>,
    /// The request's committed erasure tombstones before the retry.
    pub effects_before: usize,
    /// The request's committed erasure tombstones after the retry.
    pub effects_after: usize,
}

/// Fold one stored record, its copy rows, and its peer rows into a report.
pub(crate) fn report_of(
    env: &SubjectRequestEnv<'_>,
    record: &SubjectRequestRecord,
) -> Result<RequestStatusReport, AmError> {
    let mut pending: Vec<PendingOperatorAction> = record
        .actions
        .iter()
        .filter(|(_, action)| action.state == RequestActionState::Open)
        .map(|(id, action)| PendingOperatorAction {
            action_id: id.clone(),
            kind: Some(action.kind),
        })
        .collect();
    let copies = env.copies.load_all()?;
    for row in copies
        .iter()
        .filter(|row| row.request_id == record.request_id)
    {
        let disposition = copy_row_disposition(row)?;
        if disposition.is_open() {
            let kind = disposition
                .actions
                .iter()
                .find(|action| action.state == RequestActionState::Open)
                .map_or(RequestActionKind::Copy, |action| {
                    request_action_kind(action.kind)
                });
            pending.push(PendingOperatorAction {
                action_id: row.copy_id.clone(),
                kind: Some(kind),
            });
        }
    }
    pending.extend(pending_event_actions(env, &record.request_id)?);
    Ok(RequestStatusReport {
        request_id: record.request_id.clone(),
        kind: record.kind,
        scope: record.scope.clone(),
        state: record.state.clone(),
        technical_completion: technical_completion(&record.state),
        pending_operator_actions: pending,
        request_outcome: request_outcome(&record.state),
        peers: derived_peer_states(record, &env.peers.load_for_request(&record.request_id)?),
        peer_enablement: record.peer_enablement,
        verification: record.verification,
        deadline: record.deadline.clone(),
        exception: record.exception.clone(),
    })
}

/// The durable pending local-event actions bound to one request.
///
/// An erasure action id carries its request id, and a peer-denial event's scope
/// is its request id, so each pending action joins to its request the same way
/// the restart reconciliation joins it. An action of any other producer, or one
/// bound to another request, is left out. No `RequestActionKind` names an
/// erasure, so the erasure action's kind is left unknown (`None`), and a peer
/// denial reuses `Delivery`, the kind that names a result delivered to the
/// covered peers.
fn pending_event_actions(
    env: &SubjectRequestEnv<'_>,
    request_id: &str,
) -> Result<Vec<PendingOperatorAction>, AmError> {
    let pending = env.events.pending_actions()?;
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let events = env.events.list_events()?;
    let mut actions = Vec::new();
    for action_id in pending {
        let Some(event) = events.iter().find(|event| event.action_id == action_id) else {
            continue;
        };
        if event.kind == ProtectedAction::Erasure.producer_id()
            && request_id_of(&action_id) == Some(request_id)
        {
            actions.push(PendingOperatorAction {
                action_id,
                kind: None,
            });
        } else if event.kind == ProtectedAction::PeerDenial.producer_id()
            && event.scope == request_id
        {
            actions.push(PendingOperatorAction {
                action_id,
                kind: Some(RequestActionKind::Delivery),
            });
        }
    }
    Ok(actions)
}

fn technical_completion(state: &RequestState) -> TechnicalCompletion {
    match state.category() {
        StateCategory::Working => TechnicalCompletion::Pending,
        StateCategory::TechnicalCompletion | StateCategory::OperatorAction => {
            TechnicalCompletion::Complete
        }
        StateCategory::Exception => TechnicalCompletion::Unknown,
        StateCategory::FinalOutcome => TechnicalCompletion::Complete,
    }
}

fn request_outcome(state: &RequestState) -> RequestOutcome {
    match state {
        RequestState::Final(FinalOutcome::Completed) => RequestOutcome::Completed,
        RequestState::Final(FinalOutcome::PartiallyCompleted) => RequestOutcome::PartiallyCompleted,
        RequestState::Refused { .. } => RequestOutcome::Refused,
        RequestState::Blocked { .. } => RequestOutcome::Blocked,
        _ => RequestOutcome::Open,
    }
}

fn request_action_kind(kind: CopyRequiredAction) -> RequestActionKind {
    match kind {
        CopyRequiredAction::Erase | CopyRequiredAction::Remove => RequestActionKind::Copy,
        CopyRequiredAction::Delivery => RequestActionKind::Delivery,
        CopyRequiredAction::Notice => RequestActionKind::Notice,
    }
}
