// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the peer-denial event producer: the
//! content-free intent and outcome envelope of one peer's denial and the
//! durable writes that make the event restart-safe.
//!
//! ## Owns
//! - `peer_denial_event`, the peer-denial envelope producer
//! - `record_peer_denial_intent` and `record_peer_denial_outcome`, the durable
//!   peer-denial event writes through the local event interface
//! - `peer_outcome` and the content-free peer result texts
//!
//! ## Does not own
//! - The event envelope shape and the closed producer and pending sets (see
//!   `subject_request_events.rs`)
//! - The durable per-peer action row contract and its SQL (`clio-store`)
//! - The durable local event writer and its SQL (`clio-store`); this module
//!   only calls its interface
//! - Subject content, identity proof, key material, or tokens: no field here
//!   holds them, and the result texts stay generic
//!
//! ## Boundary
//!
//! Changes to how a peer-denial event is produced or written should normally
//! require changes here. Changes to the envelope shape or to the per-peer row
//! contract should not require changes here.
//!
//! Keep this module focused on the peer-denial event producer.

use super::{
    ACTION_EVENT_TYPE, ACTION_EVENT_VERSION, ActionEventKind, ActionIntent, ActionOutcome,
    ActionPayload, PendingResult, ProtectedAction, RequestScope, SubjectActionEvent,
};
use crate::subject_peer::peer_action_id;
use clio_store::{
    LocalEventIntent, LocalEventStore, PeerActionKind, PeerActionRow, PeerActionState,
    PeerActionStore,
};
use clio_types::{Actor, AmError, ErrorCode};

/// The content-free result text one acknowledged peer denial records.
pub(crate) const PEER_RESULT_ACKNOWLEDGED: &str = "peer denial acknowledged";
/// The content-free result text one refused peer denial records.
pub(crate) const PEER_RESULT_REFUSED: &str = "peer denial refused";

/// The durable result one peer's row supports, once the row is settled.
///
/// A pending or delivered row has no result yet, so no outcome — and no
/// acknowledgement — can rest on it.
pub(crate) fn peer_outcome(row: &PeerActionRow) -> Option<(&'static str, bool)> {
    match row.state {
        PeerActionState::Acknowledged => Some((PEER_RESULT_ACKNOWLEDGED, false)),
        PeerActionState::Refused => Some((PEER_RESULT_REFUSED, true)),
        PeerActionState::Pending | PeerActionState::Delivered => None,
    }
}

/// The content-free envelope of one peer's denial stage.
///
/// `settled` is `None` for the pre-effect intent, which is produced before the
/// denial is delivered and carries the peer's pending result. It is the peer's
/// durable row for the outcome, and the row is the only acknowledgement
/// evidence: a pending or delivered row, or a row bound to another peer
/// action, refuses, so a transport receipt or a bare send can never produce an
/// outcome. No subject prose, key material, identity proof, or the row's proof
/// reference enters the envelope.
///
/// # Errors
/// An outcome stage whose durable row is not this peer's settled denial row.
pub fn peer_denial_event(
    request_id: &str,
    peer_id: &str,
    scope: &RequestScope,
    settled: Option<&PeerActionRow>,
    actor: Actor,
    time: &str,
    sequence: u64,
) -> Result<SubjectActionEvent, AmError> {
    let action_id = peer_action_id(request_id, peer_id);
    let (kind, outcome, pending) = match settled {
        None => (
            ActionEventKind::Intent,
            None,
            Some(PendingResult::PeerDenialPending),
        ),
        Some(row)
            if row.request_id == request_id
                && row.peer_id == peer_id
                && row.action_kind == PeerActionKind::Denial
                && row.action_id == action_id =>
        {
            match row.state {
                PeerActionState::Acknowledged => {
                    (ActionEventKind::Outcome, Some(ActionOutcome::Done), None)
                }
                PeerActionState::Refused => (
                    ActionEventKind::Outcome,
                    Some(ActionOutcome::Refused {
                        reason: PEER_RESULT_REFUSED.into(),
                    }),
                    None,
                ),
                PeerActionState::Pending | PeerActionState::Delivered => {
                    return Err(AmError::new(
                        ErrorCode::Forbidden,
                        "the peer denial row is not a durable acknowledgement or refusal",
                    ));
                }
            }
        }
        Some(_) => {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "the peer denial row is bound to another action",
            ));
        }
    };
    let stage = match kind {
        ActionEventKind::Intent => "intent",
        ActionEventKind::Outcome => "outcome",
    };
    Ok(SubjectActionEvent {
        event_type: ACTION_EVENT_TYPE.into(),
        event_id: format!("{action_id}:{stage}"),
        time: time.into(),
        version: ACTION_EVENT_VERSION,
        correlation_id: request_id.into(),
        sequence,
        actor,
        payload: ActionPayload {
            producer: ProtectedAction::PeerDenial,
            kind,
            intent: ActionIntent {
                action_id,
                scope: scope.clone(),
            },
            outcome,
            pending,
        },
    })
}

/// Write the durable intent of one peer's denial before it is delivered.
///
/// The row is keyed by the peer's derived action id and scoped by the request
/// id, so a restart reconciles the same action. A repeated intent for the same
/// action is a tolerated resume and leaves the stored row as the only row. A
/// failed write refuses the peer action before any effect.
///
/// # Errors
/// An action id held by another event kind, or a storage failure.
pub fn record_peer_denial_intent(
    events: &dyn LocalEventStore,
    request_id: &str,
    peer_id: &str,
    actor: Actor,
    time: &str,
) -> Result<(), AmError> {
    let action_id = peer_action_id(request_id, peer_id);
    let kind = ProtectedAction::PeerDenial.producer_id();
    let written = events.record_intent(&LocalEventIntent {
        action_id: &action_id,
        actor: actor.as_str(),
        scope: request_id,
        kind,
        event_time: time,
    });
    match written {
        Ok(()) => Ok(()),
        Err(err) => match events
            .list_events()?
            .into_iter()
            .find(|row| row.action_id == action_id)
        {
            Some(row) if row.kind == kind => Ok(()),
            Some(_) => Err(AmError::new(
                ErrorCode::Forbidden,
                "the peer denial action id is held by another event kind",
            )),
            None => Err(err),
        },
    }
}

/// Write the durable outcome of one peer's denial after its row settled.
///
/// The per-peer row is the only evidence: a pending or delivered row refuses
/// the write, so a transport receipt or a bare send can never complete the
/// event. The write is duplicate-tolerant, so a failed outcome leaves the
/// pending intent as the one durable reconciliation item, and a later pass
/// settles the same action without a second peer effect.
///
/// # Errors
/// No durable row for the peer, an unsettled row, or a storage failure.
pub fn record_peer_denial_outcome(
    events: &dyn LocalEventStore,
    peers: &dyn PeerActionStore,
    request_id: &str,
    peer_id: &str,
) -> Result<(), AmError> {
    let action_id = peer_action_id(request_id, peer_id);
    let row = peers
        .load(request_id, peer_id, PeerActionKind::Denial)?
        .filter(|row| row.action_id == action_id)
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::NotFound,
                format!("no durable peer denial row for `{peer_id}`"),
            )
        })?;
    let (result, denied) = peer_outcome(&row).ok_or_else(|| {
        AmError::new(
            ErrorCode::Forbidden,
            "the peer denial row is not a durable acknowledgement or refusal",
        )
    })?;
    events.record_outcome(&action_id, denied, result)
}
