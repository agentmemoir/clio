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
//! - `erase_from_durable_store`, routing an erasure through the confirmed
//!   record loaded from the durable store
//!
//! ## Does not own
//! - The durable request SQL, its scope-conflict refusal, and its row
//!   validation (`clio-store`)
//! - The record vocabulary (`subject_request_frozen.rs`) and the ledger
//!   register/apply contracts (`subject_request.rs`)
//! - The bank-coordination check and its refusals
//!   (`subject_bank_coordination.rs`)
//! - The erasure workflow (`erase.rs`)
//!
//! ## Boundary
//!
//! Changes to how a request is persisted or reloaded should normally require
//! changes here. Changes to the state machine, the coordination refusal, or
//! the erasure workflow should not require changes here.
//!
//! Keep this module focused on the durable-record consumer mapping.

use crate::erase::{EraseArgs, EraseOutcome};
use crate::subject_bank_coordination::coordinated_erase_request;
use crate::subject_request::{
    RequestActionKind, RequestActionState, RequestKind, RequestScope, SubjectRequestLedger,
    SubjectRequestRecord,
};
use crate::subject_request_events::ClosureInputs;
use clio_store::{DurableRequestRow, DurableRequestStore};
use clio_types::{AmError, ErrorCode};
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
/// The named product path is known only when the record holds its path slot;
/// every copy action still open owes a durable result; a recorded retention
/// exception is the approved one.
#[must_use]
pub fn closure_inputs(record: &SubjectRequestRecord, product_path: &str) -> ClosureInputs {
    ClosureInputs {
        product_path_known: record.paths.contains_key(product_path),
        open_copy_actions: record
            .actions
            .values()
            .filter(|action| {
                action.kind == RequestActionKind::Copy && action.state == RequestActionState::Open
            })
            .count(),
        retention_exception_approved: record.exception.is_some(),
    }
}

/// Erase through the confirmed record the durable store holds.
///
/// Test and tooling seam, not the shipping command-layer path: its callers
/// are tests that persist a record first. The shipping erasure path calls
/// the erasure workflow directly without passing through this seam.
///
/// The store row is the only source of the confirmed scope and peer slots: a
/// missing row refuses, and the loaded record drives the coordination checks
/// before [`coordinated_erase_request`] runs any effect. `live_banks` must be
/// the bank set the erasure effect fans out to, read from the effect's own
/// store just before the call. A retry after a partial failure reloads the
/// same row, so the authorized scope can never widen between attempts.
///
/// # Errors
/// A missing row, a row that does not decode, a coordination refusal, or any
/// [`crate::erase::erase_request`] failure.
pub fn erase_from_durable_store(
    durable: &dyn DurableRequestStore,
    request_id: &str,
    args: &EraseArgs<'_>,
    live_banks: &[String],
) -> Result<EraseOutcome, AmError> {
    let row = durable.load(request_id)?.ok_or_else(|| {
        AmError::new(
            ErrorCode::NotFound,
            format!("no durable request {request_id}"),
        )
    })?;
    coordinated_erase_request(args, &row_to_record(&row)?, live_banks)
}

#[cfg(test)]
#[path = "subject_request_store_wiring_tests.rs"]
mod subject_request_store_wiring_tests;

#[cfg(test)]
#[path = "subject_request_store_wiring_erase_tests.rs"]
mod subject_request_store_wiring_erase_tests;

#[cfg(test)]
#[path = "subject_request_store_wiring_pg_tests.rs"]
mod subject_request_store_wiring_pg_tests;
