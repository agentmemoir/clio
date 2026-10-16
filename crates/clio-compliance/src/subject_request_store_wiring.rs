// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the consumer-side wiring between the typed
//! subject-request contract and the durable request store: the record/row
//! mapping, the restart ledger reload, the closure-fact derivation from a
//! stored record, and the erasure seam that loads the confirmed record from
//! the durable store before any effect.
//!
//! ## Owns
//! - `record_to_row` / `row_to_record`, the canonical serialization of a
//!   `SubjectRequestRecord` into a `DurableRequestRow` and back
//! - `reload_ledger`, rebuilding the ledger from the store's restart read
//! - `closure_inputs`, deriving the closure-acceptance facts from a stored
//!   record
//! - `copy_row_disposition` and `copy_row_closure_facts`, attaching every
//!   stored copy row of a request to a request result or a named open gap
//!   and folding the rows into the closure-gate facts
//! - `record_copy_disposition` and `record_retention_exception`, the
//!   privileged-actor gates for recording a copy disposition and a
//!   request-level retention exception
//! - `erase_from_durable_store`, the internal request-bound entry that loads
//!   the confirmed record from the durable store and runs the erasure retry
//!   contract
//!
//! The copy-disposition seam is a test and tooling seam, not the shipping
//! command-layer path: `copy_row_closure_facts`, `record_copy_disposition`,
//! and `record_retention_exception` have no shipping caller yet, so no
//! request path refuses on copy state through them.
//!
//! ## Does not own
//! - The durable request SQL, its scope-conflict refusal, and its row
//!   validation, and the copy-inventory SQL (`clio-store`)
//! - The record vocabulary (`subject_request_frozen.rs`) and the ledger
//!   register/apply contracts (`subject_request.rs`)
//! - The bank-coordination check and its refusals
//!   (`subject_bank_coordination.rs`)
//! - The confirmed-erasure retry contract (`erasure_retry.rs`)
//! - The erasure workflow (`erase.rs`)
//!
//! ## Boundary
//!
//! Changes to how a request is persisted or reloaded should normally require
//! changes here. Changes to the state machine, the coordination refusal, or
//! the erasure workflow should not require changes here.
//!
//! Keep this module focused on the durable-record consumer mapping.

use crate::copy_disposition::{
    CopyAction, CopyClosureFacts, CopyCustodyClass, CopyDisposition, CopyDispositionOutcome,
    CopyOwner, CopyRequiredAction, CopyState, copy_closure_facts,
};
use crate::erase::{EraseArgs, EraseOutcome};
use crate::erasure_retry::erase_confirmed_request;
use crate::local_review::authorize;
use crate::subject_request::{
    PathState, RequestActionState, RequestKind, RequestScope, SubjectRequestLedger,
    SubjectRequestRecord,
};
use crate::subject_request_events::ClosureInputs;
use clio_store::{
    CopyInventoryRow, CopyInventoryStore, DurableRequestRow, DurableRequestStore, LocalEventStore,
};
use clio_types::{Actor, AmError, ErrorCode, OpContext};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Encode one field of the durable record.
///
/// Every field is a closed struct, so serialization cannot fail; a failure
/// would be a code defect, not a storage state.
fn encode<T: Serialize>(value: &T, what: &str) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|err| panic!("durable request {what} encode: {err}"))
}

/// Decode one field of the durable record; a failure is a storage defect.
fn decode<T: DeserializeOwned>(text: &str, what: &str) -> Result<T, AmError> {
    serde_json::from_str(text).map_err(|err| {
        AmError::new(
            ErrorCode::Internal,
            format!("durable request {what} decode: {err}"),
        )
    })
}

/// Map a record onto the durable row shape.
///
/// Every column is the record's own canonical serialization: the full record
/// into the `record` column, its scope into `scope`, and its kind and state
/// into the query columns. The row round-trips through [`row_to_record`]
/// unchanged.
#[must_use]
pub fn record_to_row(record: &SubjectRequestRecord) -> DurableRequestRow {
    DurableRequestRow {
        request_id: record.request_id.clone(),
        kind: encode(&record.kind, "kind"),
        state: encode(&record.state, "state"),
        scope: encode(&record.scope, "scope"),
        deadline: record.deadline.clone(),
        record: encode(record, "record"),
    }
}

/// Decode a stored row back into its record.
///
/// The row's id must name the same request the record holds, and its kind
/// and scope columns must carry the record's own kind and scope; anything
/// else is a corrupted row and fails closed.
///
/// # Errors
/// A record the stored JSON cannot decode, or a row whose columns disagree
/// with the record they hold.
pub fn row_to_record(row: &DurableRequestRow) -> Result<SubjectRequestRecord, AmError> {
    let record: SubjectRequestRecord = decode(&row.record, "record")?;
    if record.request_id != row.request_id {
        return Err(AmError::new(
            ErrorCode::Internal,
            format!(
                "durable request row `{}` holds the record of `{}`",
                row.request_id, record.request_id
            ),
        ));
    }
    let kind: RequestKind = decode(&row.kind, "kind")?;
    let scope: RequestScope = decode(&row.scope, "scope")?;
    if kind != record.kind || scope != record.scope {
        return Err(AmError::new(
            ErrorCode::Internal,
            format!(
                "durable request row `{}` carries a kind or scope its record does not hold",
                row.request_id,
            ),
        ));
    }
    Ok(record)
}

/// Rebuild the ledger from the store's restart read.
///
/// Rows come back verbatim: nothing is auto-completed, and a request left in
/// progress stays exactly as stored.
///
/// # Errors
/// A row that does not decode, or a storage failure.
pub fn reload_ledger(store: &dyn DurableRequestStore) -> Result<SubjectRequestLedger, AmError> {
    let records = store
        .load_all()?
        .iter()
        .map(row_to_record)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SubjectRequestLedger::load(records))
}

/// The closure-acceptance facts a stored record supports.
///
/// The named product path is known only when its recorded slot is
/// [`PathState::Done`]; a pending, saved, or failed path — and an absent slot
/// — refuses technical completion. `open_copy_actions` counts every required
/// operator action still open, whether copy, notice, or delivery; a recorded
/// retention exception is the approved one.
#[must_use]
pub fn closure_inputs(record: &SubjectRequestRecord, product_path: &str) -> ClosureInputs {
    ClosureInputs {
        product_path_known: record
            .paths
            .get(product_path)
            .is_some_and(|slot| slot.state == PathState::Done),
        open_copy_actions: record
            .actions
            .values()
            .filter(|action| action.state == RequestActionState::Open)
            .count(),
        retention_exception_approved: record.exception.is_some(),
    }
}

/// Erase through the confirmed record the durable store holds, internally.
///
/// Internal request-bound erasure entry, not yet bound to any command: its
/// callers persist a record first, and the bank-scope public tool keeps its own
/// path while the crate-root entry stays available to the command layer.
///
/// The store row is the only source of the confirmed scope and peer slots: a
/// missing row refuses, and the loaded record drives the coordination
/// preflight before the retry contract runs the durable intent, the steps
/// still outstanding, and the settle. `live_banks` must be the bank set the
/// erasure effect fans out to, read from the effect's own store just before
/// the call; the flow checks it once, before the intent and any effect, so a
/// subject-bank write that lands before that read refuses the erasure, while
/// the unguarded window runs from the read to the store's own in-transaction
/// bank re-select. A retry reloads the same row, so the authorized scope can
/// never widen between attempts, and the durable record itself is never
/// rewritten.
///
/// # Errors
/// A missing row, a row that does not decode, a coordination refusal, or any
/// failure of [`crate::erasure_retry::erase_confirmed_request`].
pub fn erase_from_durable_store(
    durable: &dyn DurableRequestStore,
    events: &dyn LocalEventStore,
    peers: &dyn clio_store::PeerActionStore,
    request_id: &str,
    args: &EraseArgs<'_>,
    live_banks: &[String],
) -> Result<EraseOutcome, AmError> {
    erase_confirmed_request(durable, events, peers, request_id, args, live_banks)
}

/// The stored custody class of a copy row.
///
/// A class name the disposition vocabulary does not hold is
/// [`CopyCustodyClass::Unknown`], which refuses any completion claim.
fn custody_class(text: &str) -> CopyCustodyClass {
    match text {
        "clio" => CopyCustodyClass::Clio,
        "operator" => CopyCustodyClass::Operator,
        "caller" => CopyCustodyClass::Caller,
        "peer" => CopyCustodyClass::Peer,
        "review_output" => CopyCustodyClass::ReviewOutput,
        _ => CopyCustodyClass::Unknown,
    }
}

/// The technical erasure state of a copy row.
///
/// A state name the disposition vocabulary does not hold is
/// [`CopyState::Unknown`], which refuses a technical-completion claim.
fn copy_state(text: &str) -> CopyState {
    match text {
        "pending" => CopyState::Pending,
        "done" => CopyState::Done,
        "failed" => CopyState::Failed,
        _ => CopyState::Unknown,
    }
}

/// Parse one stored copy row into its disposition.
///
/// The row is metadata only: the disposition carries the row's custody,
/// owner, required action, and state, and no copy content. The parse fails
/// closed: an owner or a required action the vocabulary does not hold
/// refuses the row, because neither has an unknown variant, while an
/// unrecognized custody or state maps to its own unknown variant.
///
/// The row's action stays open while the copy owes a durable result: a named
/// exception reason or any technical state other than done keeps it open, so
/// the row attaches to a named open gap instead of a request result.
///
/// # Errors
/// An owner or a required action the disposition vocabulary does not hold.
pub fn copy_row_disposition(row: &CopyInventoryRow) -> Result<CopyDisposition, AmError> {
    let owner = match row.owner.as_str() {
        "harness" => CopyOwner::Harness,
        "system" => CopyOwner::System,
        other => {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!(
                    "copy `{}` names owner `{other}`, which carries no compliance privilege",
                    row.copy_id
                ),
            ));
        }
    };
    let kind = match row.required_action.as_str() {
        "erase" => CopyRequiredAction::Erase,
        "remove" => CopyRequiredAction::Remove,
        "delivery" => CopyRequiredAction::Delivery,
        "notice" => CopyRequiredAction::Notice,
        other => {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!(
                    "copy `{}` names required action `{other}`, which the vocabulary does not hold",
                    row.copy_id
                ),
            ));
        }
    };
    let state = copy_state(&row.state);
    let open = row.exception_reason.is_some() || state != CopyState::Done;
    Ok(CopyDisposition {
        copy_id: row.copy_id.clone(),
        custody: custody_class(&row.custody),
        owner,
        state,
        actions: vec![CopyAction {
            kind,
            state: if open {
                RequestActionState::Open
            } else {
                RequestActionState::Closed
            },
        }],
    })
}

/// The closure facts the stored record and its copy rows support.
///
/// Only the rows of `record`'s own request count: the restart read returns
/// every stored row, and another request's copies do not attach to this
/// record. Every attached row becomes a request result or a named open gap,
/// and the record's own open required actions count on top of the rows' open
/// dispositions, so neither source can hide an open action from the gate. An
/// outcome the rows alone would call full drops to partial while a
/// record-level required action is still open.
///
/// The two sources can describe the same action: no shared action id joins a
/// record action slot and a copy row, so the counts are summed without
/// deduplication and the value is used only as a `> 0` gate.
///
/// # Errors
/// A row whose owner or required action the vocabulary does not hold.
pub fn copy_row_closure_facts(
    record: &SubjectRequestRecord,
    rows: &[CopyInventoryRow],
    product_path: &str,
) -> Result<CopyClosureFacts, AmError> {
    let dispositions = rows
        .iter()
        .filter(|row| row.request_id == record.request_id)
        .map(copy_row_disposition)
        .collect::<Result<Vec<_>, _>>()?;
    let base = closure_inputs(record, product_path);
    let mut facts = copy_closure_facts(
        &dispositions,
        base.product_path_known,
        base.retention_exception_approved,
    );
    facts.inputs.open_copy_actions += base.open_copy_actions;
    if facts.outcome == CopyDispositionOutcome::Full && facts.inputs.open_copy_actions > 0 {
        facts.outcome = CopyDispositionOutcome::Partial;
    }
    Ok(facts)
}

/// Record one copy disposition through the privileged-actor gate.
///
/// Mirrors `record_review_output_custody`: the gate authorizes the context's
/// actor first, then the row is stored, so an operator action or a retention
/// exception can only be recorded by a harness or system actor. The row's
/// owner must name the recording actor, so one actor can never record a
/// disposition attributed to another. The row is metadata only.
///
/// # Errors
/// A non-privileged actor, an owner that names another actor, or a storage
/// failure, including the store's own reused-id refusal.
pub fn record_copy_disposition(
    ctx: &OpContext,
    store: &dyn CopyInventoryStore,
    row: &CopyInventoryRow,
) -> Result<(), AmError> {
    authorize(ctx.actor)?;
    let owner = match ctx.actor {
        Actor::Harness => "harness",
        Actor::System => "system",
        _ => {
            return Err(AmError::new(
                ErrorCode::Forbidden,
                "copy dispositions require a harness or system actor",
            ));
        }
    };
    if row.owner != owner {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "a copy disposition's owner must name the actor that recorded it",
        ));
    }
    store.save(row)
}

/// Record a request-level retention exception through the privileged gate.
///
/// Mirrors [`record_copy_disposition`]: the gate authorizes the context's
/// actor first, then the record is loaded from the durable store, its
/// `exception` slot is set, and the row is saved back. Only a harness or
/// system actor may record; a blank reason or an unknown request refuses.
/// The reason is metadata only.
///
/// # Errors
/// A non-privileged actor, a blank reason, an unknown request, or a storage
/// failure.
pub fn record_retention_exception(
    ctx: &OpContext,
    durable: &dyn DurableRequestStore,
    request_id: &str,
    reason: &str,
) -> Result<(), AmError> {
    authorize(ctx.actor)?;
    if reason.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "a retention exception reason is required",
        ));
    }
    let row = durable.load(request_id)?.ok_or_else(|| {
        AmError::new(
            ErrorCode::NotFound,
            format!("no durable request {request_id}"),
        )
    })?;
    let mut record = row_to_record(&row)?;
    record.exception = Some(reason.to_owned());
    durable.save(&record_to_row(&record))
}

#[cfg(test)]
#[path = "subject_request_store_wiring_tests.rs"]
mod subject_request_store_wiring_tests;

#[cfg(test)]
#[path = "subject_request_store_wiring_erase_tests.rs"]
mod subject_request_store_wiring_erase_tests;

// Declared here, beside the seam it drives, so the retry suites reuse this
// module's shared erasure fixtures.
#[cfg(test)]
#[path = "erasure_retry_tests.rs"]
mod erasure_retry_tests;

#[cfg(test)]
#[path = "erasure_retry_reconcile_tests.rs"]
mod erasure_retry_reconcile_tests;

#[cfg(test)]
#[path = "erasure_reconcile_guard_tests.rs"]
mod erasure_reconcile_guard_tests;

#[cfg(test)]
#[path = "subject_request_store_wiring_pg_tests.rs"]
mod subject_request_store_wiring_pg_tests;

#[cfg(test)]
#[path = "subject_request_store_wiring_copy_scenarios_tests.rs"]
mod subject_request_store_wiring_copy_scenarios_tests;

#[cfg(test)]
#[path = "subject_request_store_wiring_copy_scenarios_pg_tests.rs"]
mod subject_request_store_wiring_copy_scenarios_pg_tests;

// Declared beside the seam it drives, so the completion-guard suite reuses its fixtures.
#[cfg(test)]
#[path = "erasure_retry_completion_guard_tests.rs"]
mod erasure_retry_completion_guard_tests;
