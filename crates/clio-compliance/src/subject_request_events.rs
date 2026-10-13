// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-request action-event contract:
//! the stable event envelope, the protected-action event producers, the
//! durable pending results, the public-binding gate, the restore refusal, the
//! closure acceptance check, and an in-memory append-only event writer.
//!
//! ## Owns
//! - `SubjectActionEvent` stable envelope and `ActionPayload`
//! - `ProtectedAction` (the export, erasure, and restore event producers),
//!   `ActionIntent`, `ActionOutcome`, and `PendingResult`
//! - `check_binding` public-binding gate and `BindingRefusal`
//! - `check_restore` restore gate and `RestoreRefusal`
//! - `accept_closure` closure acceptance and `ClosureBlock`
//! - `ActionEventLog` in-memory append-only event writer
//!
//! ## Does not own
//! - The request state table and ledger (see `subject_request.rs`)
//! - Copy-custody manifest fields (see `subject_request_manifest.rs`)
//! - Any storage adapter, SQL, I/O, or protected action performed: the log is
//!   in-memory only, and the gates are pure functions
//! - Subject content, identity proof, key material, or tokens: no field here
//!   holds them, and free-text reasons must stay generic
//!
//! ## Boundary
//!
//! Changes to event shape, producer ids, gate rules, or refusal reasons should
//! normally require changes here. Changes to request state semantics or to how
//! events are stored should not require changes here.
//!
//! Keep this module focused on the pure action-event contract.

use crate::subject_request::{FinalOutcome, RequestScope};
use crate::subject_request_manifest::is_rfc3339_utc;
use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The stable event type string carried by every action event envelope.
pub const ACTION_EVENT_TYPE: &str = "compliance.subject_request.action";

/// The stable envelope version.
pub const ACTION_EVENT_VERSION: u32 = 1;

/// The protected actions that produce subject-request events: the copy export
/// producer, the verified erasure producer, and the guarded restore producer.
///
/// The set is closed: a new producer is a contract change, never a variant a
/// consumer may ignore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedAction {
    /// The copy export producer.
    CopyExport,
    /// The verified erasure producer.
    Erasure,
    /// The guarded restore producer.
    Restore,
}

impl ProtectedAction {
    /// The stable producer id serialized into events.
    #[must_use]
    pub const fn producer_id(self) -> &'static str {
        match self {
            Self::CopyExport => "copy_export",
            Self::Erasure => "erasure",
            Self::Restore => "restore",
        }
    }
}

/// Pre-effect intent: what a protected action is about to do, recorded before
/// any effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionIntent {
    /// The durable action id; one intent per action id.
    pub action_id: String,
    /// The scope the action applies to.
    pub scope: RequestScope,
}

/// Post-effect outcome: what a protected action actually did.
///
/// Free-text reasons must stay generic and never carry subject content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    /// The action finished and owes no durable result.
    Done,
    /// The action was refused before any effect.
    Refused {
        /// Why the action was refused; must stay generic.
        reason: String,
    },
    /// The action stopped on an exception.
    Failed {
        /// What stopped the action; must stay generic.
        reason: String,
    },
}

/// Durable pending result: what a protected action still owes before it is
/// done.
///
/// The set is closed: a new obligation is a contract change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingResult {
    /// The copy-custody manifest for the copy is not yet written.
    CopyManifestPending,
    /// The post-effect outcome has not been recorded yet.
    OutcomePending,
}

/// One protected-action event for a subject request.
///
/// The envelope is stable: type, id, time, version, and correlation id never
/// change meaning. The payload carries the action's producer, its action id,
/// its pre-effect intent, its post-effect outcome, and its durable pending
/// result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectActionEvent {
    /// Envelope event type; always [`ACTION_EVENT_TYPE`].
    pub event_type: String,
    /// Durable event id; recording the same id twice replays the stored event.
    pub event_id: String,
    /// RFC 3339 UTC event time.
    pub time: String,
    /// Envelope version; always [`ACTION_EVENT_VERSION`].
    pub version: u32,
    /// Correlation id: the durable request id the event belongs to.
    pub correlation_id: String,
    /// The event payload.
    pub payload: ActionPayload,
}

/// Event-specific payload of [`SubjectActionEvent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionPayload {
    /// The protected action that produced the event.
    pub producer: ProtectedAction,
    /// The pre-effect intent.
    pub intent: ActionIntent,
    /// The post-effect outcome; absent while the action is still running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ActionOutcome>,
    /// The durable pending result; absent once the action owes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingResult>,
}

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
    Ok(())
}

/// The controls a public binding must hold before its gated actions run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BindingControls {
    /// The copy controls are accepted.
    pub copy_controls_accepted: bool,
    /// The guarded restore command is ready.
    pub guarded_restore_ready: bool,
    /// The erasure commands are ready.
    pub erasure_commands_ready: bool,
}

/// Why a gated action is refused on a public binding.
///
/// The set is closed: a new reason is a contract change, and a consumer must
/// not guess an unknown string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingRefusal {
    /// Subject export waits for the copy controls.
    CopyControlsNotAccepted,
    /// Request-bound erasure waits for the guarded restore.
    GuardedRestoreNotReady,
    /// Request-bound erasure waits for the erasure commands.
    ErasureCommandsNotReady,
}

/// The public-binding gate.
///
/// Subject selection and request-bound erasure stay unavailable on public
/// bindings until their controls are accepted: subject export waits for the
/// copy controls, and request-bound erasure waits for the guarded restore
/// and the erasure commands. A bank-scope action is not gated here, and a
/// subject-scope restore is not gated here either: restore is gated by
/// [`check_restore`] instead.
pub fn check_binding(
    scope: &RequestScope,
    action: ProtectedAction,
    controls: &BindingControls,
) -> Result<(), BindingRefusal> {
    if !matches!(scope, RequestScope::Subject { .. }) {
        return Ok(());
    }
    match action {
        ProtectedAction::CopyExport if !controls.copy_controls_accepted => {
            Err(BindingRefusal::CopyControlsNotAccepted)
        }
        ProtectedAction::Erasure if !controls.guarded_restore_ready => {
            Err(BindingRefusal::GuardedRestoreNotReady)
        }
        ProtectedAction::Erasure if !controls.erasure_commands_ready => {
            Err(BindingRefusal::ErasureCommandsNotReady)
        }
        _ => Ok(()),
    }
}

/// Why a restore is refused.
///
/// The set is closed: a new reason is a contract change, and a consumer must
/// not guess an unknown string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreRefusal {
    /// No copy-custody manifest backs the restore.
    CopyManifestMissing,
    /// The copy digest does not match the digest the manifest recorded.
    CopyDigestMismatch,
    /// The receiver named by the restore is not the manifest's receiver.
    ReceiverMismatch,
}

/// The facts a restore is checked against before it acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreInputs {
    /// A copy-custody manifest exists for the action.
    pub manifest_present: bool,
    /// The copy digest matches the digest the manifest recorded.
    pub digest_verified: bool,
    /// The restoring receiver matches the manifest's receiver identity.
    pub receiver_matches: bool,
}

/// The restore gate: a restore acts only on a present, digest-verified copy
/// whose manifest receiver matches the restorer.
pub fn check_restore(inputs: &RestoreInputs) -> Result<(), RestoreRefusal> {
    if !inputs.manifest_present {
        return Err(RestoreRefusal::CopyManifestMissing);
    }
    if !inputs.digest_verified {
        return Err(RestoreRefusal::CopyDigestMismatch);
    }
    if !inputs.receiver_matches {
        return Err(RestoreRefusal::ReceiverMismatch);
    }
    Ok(())
}

/// Why closure of a request is blocked.
///
/// The set is closed: a new reason is a contract change, and a consumer must
/// not guess an unknown string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosureBlock {
    /// The product path the closure names is unknown to this build; this
    /// blocks marking the request technically complete.
    UnknownProductPath,
    /// A copy action of the request is still open; this blocks closure.
    OpenCopyAction,
}

/// The facts closure acceptance is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClosureInputs {
    /// The requested product path is known to this build.
    pub product_path_known: bool,
    /// Copy actions of the request that still owe a durable result.
    pub open_copy_actions: usize,
    /// An approved retention exception applies to the request.
    pub retention_exception_approved: bool,
}

/// The closure acceptance check.
///
/// An unknown product path blocks marking the request technically complete;
/// an open copy action blocks closure; an approved retention exception yields
/// a partial final outcome instead of a complete one.
pub fn accept_closure(inputs: &ClosureInputs) -> Result<FinalOutcome, ClosureBlock> {
    if !inputs.product_path_known {
        return Err(ClosureBlock::UnknownProductPath);
    }
    if inputs.open_copy_actions > 0 {
        return Err(ClosureBlock::OpenCopyAction);
    }
    if inputs.retention_exception_approved {
        Ok(FinalOutcome::PartiallyCompleted)
    } else {
        Ok(FinalOutcome::Completed)
    }
}
