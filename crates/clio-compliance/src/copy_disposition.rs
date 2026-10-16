// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the copy-disposition vocabulary and the
//! fold that turns recorded copy dispositions into the closure-gate facts:
//! where a known subject-data copy is held, which privileged actor recorded
//! its disposition, what action is still required on it, its technical
//! erasure state, and the erasure outcome the copies support.
//!
//! ## Owns
//! - `CopyCustodyClass` copy custody classes, including peers and
//!   review-output copies
//! - `CopyOwner` privileged-actor naming for disposition records
//! - `CopyRequiredAction`, `CopyAction`, and `CopyState`
//! - `CopyDisposition` one recorded copy disposition
//! - `CopyDispositionOutcome` and `CopyClosureFacts`
//! - `copy_closure_facts` the fail-closed fold into closure-gate facts
//! - `peer_copy_disposition`, the mapping of one peer's recorded denial
//!   result onto the same vocabulary
//!
//! ## Does not own
//! - The closure gate itself and its `FinalOutcome` decision (see
//!   `subject_request_events.rs`); this module only produces the facts the
//!   gate consumes, and `copy_row_closure_facts` in
//!   `subject_request_store_wiring.rs` connects them for a stored record
//! - Copy-custody manifest fields (see `subject_request_manifest.rs`)
//! - The durable copy-inventory store, any storage adapter, SQL, or I/O:
//!   everything here is pure and stateless
//! - Subject content, file bytes, data digests, key material, or tokens: no
//!   field here holds them, only custody metadata
//!
//! ## Boundary
//!
//! Changes to the disposition vocabulary or the fold rules should normally
//! require changes here. Changes to closure acceptance itself, to request
//! state, or to how dispositions are stored should not require changes here.
//!
//! Keep this module focused on the pure copy-disposition contract.

use crate::subject_request::RequestActionState;
use crate::subject_request_events::ClosureInputs;
use clio_store::PeerActionDisposition;
use serde::{Deserialize, Serialize};

/// Where a known subject-data copy is held.
///
/// The class names reuse the copy-ledger custody vocabulary. Peer copies and
/// review-output copies are first-class classes: an erasure claim must say
/// where every known copy sits. `Unknown` custody blocks any complete-scope
/// claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyCustodyClass {
    /// The live Clio store holds the copy.
    Clio,
    /// An operator-held location holds it.
    Operator,
    /// A caller-held copy holds it.
    Caller,
    /// A selected peer node holds it.
    Peer,
    /// A review-output copy produced by Clio holds it.
    ReviewOutput,
    /// The holder is not known; this refuses any completion claim.
    Unknown,
}

/// The privileged actor that recorded a copy disposition.
///
/// Only harness and system actors carry compliance privilege, so only they
/// record dispositions. The naming mirrors the erasure authorization gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyOwner {
    /// The test harness actor recorded the disposition.
    Harness,
    /// The system actor recorded the disposition.
    System,
}

/// The action still required on a copy before any complete-erasure claim.
///
/// Delivery and required notice are separate from technical success: a copy
/// can be technically erased while its delivery or notice action is still
/// open. Explicit removal of an operator- or peer-held copy is an operator
/// action, never an acknowledgement that delivery happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyRequiredAction {
    /// Destroy the copy under Clio custody.
    Erase,
    /// Explicit operator removal of an operator-, caller-, or peer-held copy.
    Remove,
    /// Deliver the result to the covered peers.
    Delivery,
    /// Give the required notice of the result.
    Notice,
}

/// The open/closed state of one required copy action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyAction {
    /// What the action does.
    pub kind: CopyRequiredAction,
    /// Whether the action is still owed.
    pub state: RequestActionState,
}

/// The technical erasure state of one copy.
///
/// The names mirror the covered-path state vocabulary. A copy in any state
/// other than `Done` refuses a technical-completion claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyState {
    /// No verified erasure result yet.
    Pending,
    /// The copy has a verified erasure result.
    Done,
    /// The erasure failed and needs the operator.
    Failed,
    /// No state is known; this refuses a technical-completion claim.
    Unknown,
}

/// One recorded copy disposition: metadata only, never subject content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyDisposition {
    /// Stable id naming the copy; metadata only.
    pub copy_id: String,
    /// Where the copy is held.
    pub custody: CopyCustodyClass,
    /// The privileged actor that recorded the disposition.
    pub owner: CopyOwner,
    /// The technical erasure state of the copy.
    pub state: CopyState,
    /// The required actions attached to the copy.
    pub actions: Vec<CopyAction>,
}

impl CopyDisposition {
    /// Whether the copy still owes a durable result.
    ///
    /// Fail closed: unknown custody owes a classification, any state other
    /// than `Done` owes a technical result, and any open action — including
    /// delivery or notice on a technically erased copy — is still owed.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.custody == CopyCustodyClass::Unknown
            || self.state != CopyState::Done
            || self
                .actions
                .iter()
                .any(|a| a.state == RequestActionState::Open)
    }
}

/// The erasure outcome a set of copy dispositions supports.
///
/// The set is closed: a new outcome is a contract change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyDispositionOutcome {
    /// Every copy is resolved under known custody and no exception applies.
    Full,
    /// Erasure is partial: an approved retention exception applies, or some
    /// copies still owe a result. Never a full-closure claim.
    Partial,
    /// A copy's custody is unknown, so the scope cannot be classified and no
    /// completion claim of any kind is supported.
    Refused,
}

/// The closure-gate facts a set of copy dispositions folds into.
///
/// `inputs` is field-compatible with the closure gate: the gate refuses an
/// unknown product path and any open copy action, and an approved retention
/// exception yields a partial outcome, never full closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyClosureFacts {
    /// The closure-gate inputs, in the exact shape the gate consumes.
    pub inputs: ClosureInputs,
    /// The erasure outcome the copies support; never `Full` on unknown
    /// custody, open work, or an approved exception.
    pub outcome: CopyDispositionOutcome,
}

/// Fold a set of copy dispositions into the closure-gate facts.
///
/// A copy counts as an open copy action while it owes a durable result, so
/// unknown custody and open required actions — including open delivery or
/// notice on a technically erased copy — block any full-completion claim
/// through the gate's open-action rule. `product_path_known` and
/// `retention_exception_approved` are request-level facts passed through so
/// the returned inputs can go straight to the gate.
#[must_use]
pub fn copy_closure_facts(
    dispositions: &[CopyDisposition],
    product_path_known: bool,
    retention_exception_approved: bool,
) -> CopyClosureFacts {
    let open_copy_actions = dispositions.iter().filter(|d| d.is_open()).count();
    let unknown_custody = dispositions
        .iter()
        .any(|d| d.custody == CopyCustodyClass::Unknown);
    let outcome = if unknown_custody {
        CopyDispositionOutcome::Refused
    } else if open_copy_actions > 0 || retention_exception_approved {
        CopyDispositionOutcome::Partial
    } else {
        CopyDispositionOutcome::Full
    };
    CopyClosureFacts {
        inputs: ClosureInputs {
            product_path_known,
            open_copy_actions,
            retention_exception_approved,
        },
        outcome,
    }
}

/// The copy disposition one peer's recorded denial result supports.
///
/// A peer's denial row acknowledged durably, or a row that records the peer's
/// deletion evidence with its proof reference, is a resolved copy: the
/// technical state is done and the removal action is closed. An approved
/// retention and an unknown disposition are not removal evidence: the copy
/// keeps an open removal action, and an unknown disposition also keeps its
/// technical state unknown, so [`copy_closure_facts`] yields a partial or
/// refused outcome instead of a full completion claim. The peer holds the
/// copy in every case, so the custody class stays known.
#[must_use]
pub fn peer_copy_disposition(
    peer_id: &str,
    disposition: PeerActionDisposition,
    proof_ref: Option<&str>,
) -> CopyDisposition {
    let removed = disposition == PeerActionDisposition::RemovalEvidence && proof_ref.is_some();
    let (state, action) = if removed {
        (CopyState::Done, RequestActionState::Closed)
    } else if disposition == PeerActionDisposition::RetentionApproved {
        (CopyState::Pending, RequestActionState::Open)
    } else {
        (CopyState::Unknown, RequestActionState::Open)
    };
    CopyDisposition {
        copy_id: format!("peer:{peer_id}"),
        custody: CopyCustodyClass::Peer,
        owner: CopyOwner::System,
        state,
        actions: vec![CopyAction {
            kind: CopyRequiredAction::Remove,
            state: action,
        }],
    }
}

#[cfg(test)]
#[path = "copy_disposition_tests.rs"]
mod copy_disposition_tests;
