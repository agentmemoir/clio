// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the in-memory append-only writer of
//! subject-request action events: validated recording, id replay, the
//! immutable event-id refusal, restart reload, and open-copy counting.
//!
//! ## Owns
//! - `ActionEventLog` and `RecordOutcome`
//! - The record-time envelope validation applied before an event is appended
//!
//! ## Does not own
//! - The event envelope shape and the action gates (see
//!   `subject_request_events.rs`)
//! - Any storage adapter, SQL, or I/O: the log is in-memory only
//!
//! ## Boundary
//!
//! Changes to the record, replay, refuse, reload, or open-copy counting rules
//! should normally require changes here. Changes to the event shape or to the
//! gate rules should not require changes here.
//!
//! Keep this module focused on the in-memory event writer.

use crate::subject_request_events::{
    ACTION_EVENT_TYPE, ACTION_EVENT_VERSION, ActionEventKind, ActionOutcome, ProtectedAction,
    SubjectActionEvent,
};
use crate::subject_request_manifest::is_rfc3339_utc;
use clio_types::{AmError, ErrorCode};
use std::collections::BTreeMap;

/// What [`ActionEventLog::record`] did with an event id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    /// The event was new and was appended.
    Recorded,
    /// The id and content matched a stored event: nothing was applied.
    Replayed,
}

/// In-memory append-only log of subject-request action events: the event
/// writer interface of this contract.
///
/// The log owns no persistence: a caller reloads [`SubjectActionEvent`]s from
/// its store and calls [`Self::load`] to model a restart. Events are immutable
/// once written: the same id with the same content replays, and the same id
/// with changed content is refused.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ActionEventLog {
    events: Vec<SubjectActionEvent>,
}

impl ActionEventLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild a log from events reloaded after a restart.
    ///
    /// Stored events were validated when they were recorded, so they are
    /// trusted here. Callers must not reload duplicate event ids: load keeps
    /// entries as given with no deduplication, so a duplicated id is counted
    /// once per entry it appears in.
    #[must_use]
    pub fn load(events: Vec<SubjectActionEvent>) -> Self {
        Self { events }
    }

    /// Every stored event, in record order.
    #[must_use]
    pub fn events(&self) -> Vec<SubjectActionEvent> {
        self.events.clone()
    }

    /// Append an event, or replay its stored result.
    ///
    /// A malformed event is refused and changes nothing. The same event id
    /// with the same content replays and applies nothing; the same event id
    /// with changed content is refused, because a written event is immutable.
    pub fn record(&mut self, event: SubjectActionEvent) -> Result<RecordOutcome, AmError> {
        validate(&event)?;
        if let Some(stored) = self
            .events
            .iter()
            .find(|stored| stored.event_id == event.event_id)
        {
            if *stored == event {
                return Ok(RecordOutcome::Replayed);
            }
            return Err(AmError::new(
                ErrorCode::Forbidden,
                "event id is already bound to a different event",
            ));
        }
        self.events.push(event);
        Ok(RecordOutcome::Recorded)
    }

    /// How many copy actions of a request are still open.
    ///
    /// A copy action is open when its latest event does not record a finished
    /// action with nothing still owed: no outcome yet, a pending result still
    /// owed, or an outcome other than done. A failed or refused copy that
    /// still owes disposition must therefore carry a pending result, or it
    /// stays open and keeps blocking closure.
    #[must_use]
    pub fn open_copy_actions(&self, request_id: &str) -> usize {
        let mut latest: BTreeMap<&str, &SubjectActionEvent> = BTreeMap::new();
        for event in &self.events {
            if event.payload.producer == ProtectedAction::CopyExport
                && event.correlation_id == request_id
            {
                latest.insert(event.payload.intent.action_id.as_str(), event);
            }
        }
        latest
            .values()
            .filter(|event| {
                !matches!(event.payload.outcome, Some(ActionOutcome::Done))
                    || event.payload.pending.is_some()
            })
            .count()
    }
}

fn validate(event: &SubjectActionEvent) -> Result<(), AmError> {
    if event.event_type != ACTION_EVENT_TYPE {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "event type must be the stable action-event type",
        ));
    }
    if event.version != ACTION_EVENT_VERSION {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "event version must be the stable envelope version",
        ));
    }
    if event.event_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "event id must not be blank",
        ));
    }
    if event.correlation_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "event correlation id must not be blank",
        ));
    }
    if !is_rfc3339_utc(&event.time) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "event time must be an RFC 3339 UTC timestamp",
        ));
    }
    if event.payload.intent.action_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "action id must not be blank",
        ));
    }
    event.payload.intent.scope.validate()?;
    if event.payload.outcome.is_some() != (event.payload.kind == ActionEventKind::Outcome) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "kind and outcome must agree",
        ));
    }
    Ok(())
}
