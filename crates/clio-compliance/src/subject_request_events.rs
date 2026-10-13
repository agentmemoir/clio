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
//!   `ActionIntent`, `ActionOutcome`, `ActionEventKind`, and `PendingResult`
//! - `check_binding` public-binding gate and `BindingRefusal`
//! - `check_restore` restore gate and `RestoreRefusal`
//! - `accept_closure` closure acceptance and `ClosureBlock`
//!
//! ## Does not own
//! - The request state table and ledger (see `subject_request.rs`)
//! - Copy-custody manifest fields (see `subject_request_manifest.rs`)
//! - The in-memory event writer (see `action_event_log.rs`)
//! - Any storage adapter, SQL, I/O, or protected action performed: the gates
//!   are pure functions and nothing here is stateful
//! - Subject content, identity proof, key material, or tokens: no field here
//!   holds them, and free-text reasons must stay generic
//!
//! ## Boundary
//!
//! Changes to event shape, producer ids, gate rules, or refusal reasons should
//! normally require changes here. Changes to request state semantics or to how
//! events are recorded or stored should not require changes here.
//!
//! Keep this module focused on the pure action-event contract.

use crate::subject_request::{FinalOutcome, RequestScope};
use clio_types::Actor;
use serde::{Deserialize, Serialize};

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

/// Which stage of an action an event records; the set is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionEventKind {
    /// The pre-effect intent.
    Intent,
    /// The post-effect outcome.
    Outcome,
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
/// The envelope is stable: type, id, time, version, sequence, actor, and
/// correlation id never change meaning. The payload carries the producer,
/// the event kind, the intent, the outcome, and the pending result.
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
    /// The writer-assigned sequence position; gap-free per durable store.
    pub sequence: u64,
    /// The actor that performed the action.
    pub actor: Actor,
    /// The event payload.
    pub payload: ActionPayload,
}

/// Event-specific payload of [`SubjectActionEvent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionPayload {
    /// The protected action that produced the event.
    pub producer: ProtectedAction,
    /// Which stage of the action this event records.
    pub kind: ActionEventKind,
    /// The pre-effect intent.
    pub intent: ActionIntent,
    /// The post-effect outcome; absent while the action is still running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ActionOutcome>,
    /// The durable pending result; absent once the action owes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingResult>,
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

#[cfg(test)]
#[path = "subject_request_events_gates_tests.rs"]
mod subject_request_events_gates_tests;

#[cfg(test)]
#[path = "subject_request_events_store_tests.rs"]
mod subject_request_events_store_tests;
