// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module sequences the ledger, the durable request store, the event
//! store, the peer store, and the copy store into the request verbs a binding
//! calls, and re-exports the separated status report folded from one record.
//!
//! ## Owns
//! - `SubjectRequestEnv` and the request verbs `open`, `report`,
//!   `apply_command`, `close`, `retry_erasure`, `peer_retry`, `record_copy`,
//!   `record_exception`, `review_events`, `peer_report`, `peer_disposition`,
//!   `derive_peer_enablement`
//! - `load_record` and `save_record`
//!
//! ## Does not own
//! - The separated status report and its fold (`subject_request_report.rs`)
//! - The request state table, commands, and ledger contracts
//!   (`subject_request.rs`)
//! - The record vocabulary and the record/row mapping
//!   (`subject_request_frozen.rs`, `subject_request_store_wiring.rs`)
//! - The state-machine storage on disk (`clio-store`) and any binding over it
//!
//! ## Boundary
//!
//! Changes to how the request verbs are sequenced should normally require
//! changes here; changes to the report fold, the state machine, the durable row
//! contracts, or a binding should not.
//!
//! Keep this module focused on the thin request-verb sequencing.

use crate::erase::EraseArgs;
use crate::erasure_reconcile::{ErasureRetryEnv, reconcile_erasure_actions};
use crate::local_review::{LocalReviewPage, LocalReviewQuery, authorize, review_local_events};
use crate::subject_peer::{derived_peer_states, record_disposition, snapshot_peers};
use crate::subject_request::{
    PeerEnablement, PeerState, RegisterOutcome, RequestCommand, RequestKind, RequestScope,
    SubjectRequestLedger, SubjectRequestRecord, VerificationResult,
};
use crate::subject_request_events::{ClosureBlock, accept_closure};
use crate::subject_request_report::report_of;
use crate::subject_request_store_wiring::{
    closure_inputs, copy_row_closure_facts, erase_from_durable_store, peer_row_closure_facts,
    record_copy_disposition, record_retention_exception, record_to_row, reload_ledger,
    row_to_record,
};
use clio_store::{
    CopyInventoryRow, CopyInventoryStore, DurableRequestStore, LocalEventStore,
    PeerActionDisposition, PeerActionStore,
};
use clio_types::{AmError, ErrorCode, OpContext};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub use crate::subject_request_report::{
    PendingOperatorAction, RequestOutcome, RequestStatusReport, RetryReport, TechnicalCompletion,
};

/// The borrowed store handles one request verb sequences over.
pub struct SubjectRequestEnv<'a> {
    /// The durable request store the record is loaded from and saved to.
    pub durable: &'a dyn DurableRequestStore,
    /// The local event store the review read and the erasure intent use.
    pub events: &'a dyn LocalEventStore,
    /// The per-peer action store the peer snapshot and rows use.
    pub peers: &'a dyn PeerActionStore,
    /// The copy-inventory store the copy rows are read from.
    pub copies: &'a dyn CopyInventoryStore,
}

/// Open a request, or replay its stored result, then report it.
///
/// The operator's verification decision and optional deadline are recorded on
/// a new request; a replayed id keeps its stored terms.
// Ten inputs mirror the bound scope, peer set, and terms the caller holds.
#[allow(clippy::too_many_arguments)]
pub fn open(
    env: &SubjectRequestEnv<'_>,
    _ctx: &OpContext,
    request_id: &str,
    kind: RequestKind,
    scope: &RequestScope,
    enablement: PeerEnablement,
    peers: &[&str],
    peer_banks: &[&str],
    verification: Option<VerificationResult>,
    deadline: Option<String>,
) -> Result<RequestStatusReport, AmError> {
    let mut ledger = reload_ledger(env.durable)?;
    let outcome = ledger.register_with_terms(
        request_id,
        kind,
        scope,
        enablement,
        peers,
        peer_banks,
        verification,
        deadline,
    )?;
    save_record(env, &ledger, request_id)?;
    let record = load_record(env, request_id)?;
    snapshot_peers(env.peers, &record)?;
    if matches!(outcome, RegisterOutcome::Opened) {
        ledger.apply(request_id, &RequestCommand::Begin)?;
        save_record(env, &ledger, request_id)?;
    }
    report_of(env, &load_record(env, request_id)?)
}

/// Report the separated status of one stored request.
pub fn report(
    env: &SubjectRequestEnv<'_>,
    _ctx: &OpContext,
    request_id: &str,
) -> Result<RequestStatusReport, AmError> {
    report_of(env, &load_record(env, request_id)?)
}

/// Apply one command to the stored request and report it.
pub fn apply_command(
    env: &SubjectRequestEnv<'_>,
    _ctx: &OpContext,
    request_id: &str,
    command: &RequestCommand,
) -> Result<RequestStatusReport, AmError> {
    let mut ledger = reload_ledger(env.durable)?;
    ledger.apply(request_id, command)?;
    save_record(env, &ledger, request_id)?;
    report_of(env, &load_record(env, request_id)?)
}

/// Close one request: accept the closure, move it to final, and report it.
pub fn close(
    env: &SubjectRequestEnv<'_>,
    _ctx: &OpContext,
    request_id: &str,
    product_path: &str,
) -> Result<RequestStatusReport, AmError> {
    let record = load_record(env, request_id)?;
    if record.paths.is_empty() {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "request closure refused: no product path is recorded on this request",
        ));
    }
    let copy_rows = env.copies.load_all()?;
    let peer_rows = env.peers.load_for_request(request_id)?;
    let base = closure_inputs(&record, product_path);
    let copy_facts = copy_row_closure_facts(&record, &copy_rows, product_path)?;
    let peer_facts = peer_row_closure_facts(&record, &peer_rows, product_path);
    let mut inputs = copy_facts.inputs;
    inputs.open_copy_actions += peer_facts
        .inputs
        .open_copy_actions
        .saturating_sub(base.open_copy_actions);
    let outcome = accept_closure(&inputs).map_err(closure_refusal)?;
    let mut ledger = reload_ledger(env.durable)?;
    ledger.apply(request_id, &RequestCommand::MarkTechnicalComplete)?;
    ledger.apply(request_id, &RequestCommand::AwaitOperator)?;
    ledger.apply(request_id, &RequestCommand::Finalize { outcome })?;
    save_record(env, &ledger, request_id)?;
    report_of(env, &load_record(env, request_id)?)
}

/// Retry a confirmed erasure and report the effect counts it moved.
///
/// `effects_before`/`effects_after` are committed erasure tombstones read
/// around the retry; `action_id` names the action while it was pending.
pub fn retry_erasure(
    env: &SubjectRequestEnv<'_>,
    _ctx: &OpContext,
    request_id: &str,
    args: &EraseArgs<'_>,
    live_banks: &[String],
) -> Result<RetryReport, AmError> {
    let action = crate::erasure_retry::erasure_action_id(request_id);
    let pending = env.events.pending_actions()?;
    let action_id = pending.into_iter().find(|id| *id == action);
    let effects_before = args
        .store
        .read_tombstones(args.ctx.bank.as_str(), request_id)?
        .len();
    erase_from_durable_store(
        env.durable,
        env.events,
        env.peers,
        request_id,
        args,
        live_banks,
    )?;
    let effects_after = args
        .store
        .read_tombstones(args.ctx.bank.as_str(), request_id)?
        .len();
    Ok(RetryReport {
        report: report_of(env, &load_record(env, request_id)?)?,
        action_id,
        effects_before,
        effects_after,
    })
}

/// Reconcile the pending peer actions a restart left, then report the request.
pub fn peer_retry(
    env: &SubjectRequestEnv<'_>,
    _ctx: &OpContext,
    request_id: &str,
    retry: &ErasureRetryEnv<'_>,
) -> Result<RequestStatusReport, AmError> {
    reconcile_erasure_actions(env.durable, env.events, retry, &|_| None)?;
    report_of(env, &load_record(env, request_id)?)
}

/// Record one copy disposition and report the request it names.
pub fn record_copy(
    env: &SubjectRequestEnv<'_>,
    ctx: &OpContext,
    row: &CopyInventoryRow,
) -> Result<RequestStatusReport, AmError> {
    record_copy_disposition(ctx, env.copies, row)?;
    report_of(env, &load_record(env, &row.request_id)?)
}

/// Record a request-level retention exception and report it.
pub fn record_exception(
    env: &SubjectRequestEnv<'_>,
    ctx: &OpContext,
    request_id: &str,
    reason: &str,
) -> Result<RequestStatusReport, AmError> {
    record_retention_exception(ctx, env.durable, request_id, reason)?;
    report_of(env, &load_record(env, request_id)?)
}

/// Read one content-free page of the local events bound to one request.
pub fn review_events(
    env: &SubjectRequestEnv<'_>,
    ctx: &OpContext,
    request_id: &str,
    limit: usize,
    cursor: Option<&str>,
) -> Result<LocalReviewPage, AmError> {
    let query = LocalReviewQuery {
        limit,
        cursor: cursor.map(str::to_owned),
        request_id: Some(request_id.to_owned()),
    };
    review_local_events(ctx, env.events, &query)
}

/// The peer state slots derived from the durable rows of one request.
pub fn peer_report(
    env: &SubjectRequestEnv<'_>,
    _ctx: &OpContext,
    request_id: &str,
) -> Result<BTreeMap<String, PeerState>, AmError> {
    let record = load_record(env, request_id)?;
    let rows = env.peers.load_for_request(request_id)?;
    Ok(derived_peer_states(&record, &rows))
}

/// Record one peer disposition and report the request it names.
///
/// Only a harness or system actor may record a peer disposition, the same
/// privileged-actor gate the copy-disposition path enforces.
pub fn peer_disposition(
    env: &SubjectRequestEnv<'_>,
    ctx: &OpContext,
    request_id: &str,
    peer_id: &str,
    disposition: PeerActionDisposition,
    proof_ref: Option<&str>,
) -> Result<RequestStatusReport, AmError> {
    authorize(ctx.actor)?;
    record_disposition(env.peers, request_id, peer_id, disposition, proof_ref)?;
    report_of(env, &load_record(env, request_id)?)
}

/// Derive the deployment's peer-enablement fact from its enabled records.
pub fn derive_peer_enablement(
    _effective: &Value,
    get: &dyn Fn(&str) -> Option<String>,
    single_host: bool,
) -> Result<(PeerEnablement, Vec<String>, BTreeSet<String>), AmError> {
    if single_host {
        return Ok((PeerEnablement::NotApplicable, Vec::new(), BTreeSet::new()));
    }
    let path =
        clio_config::auth_tokens::resolve_token_store_path_with(None, get).ok_or_else(|| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                "no credential store path could be resolved",
            )
        })?;
    let records = clio_config::auth_tokens::load_auth_tokens(&path)?;
    let mut labels = Vec::with_capacity(records.len());
    let mut banks = BTreeSet::new();
    for record in records {
        labels.push(record.label);
        banks.extend(record.banks);
    }
    if labels.is_empty() {
        return Ok((PeerEnablement::NotApplicable, labels, banks));
    }
    Ok((PeerEnablement::Required, labels, banks))
}

/// Load one record from the durable store, refusing when it is absent.
fn load_record(
    env: &SubjectRequestEnv<'_>,
    request_id: &str,
) -> Result<SubjectRequestRecord, AmError> {
    let row = env.durable.load(request_id)?.ok_or_else(|| {
        AmError::new(
            ErrorCode::NotFound,
            format!("no durable request {request_id}"),
        )
    })?;
    row_to_record(&row)
}

fn save_record(
    env: &SubjectRequestEnv<'_>,
    ledger: &SubjectRequestLedger,
    request_id: &str,
) -> Result<(), AmError> {
    let record = ledger
        .record(request_id)
        .ok_or_else(|| AmError::new(ErrorCode::NotFound, format!("no request {request_id}")))?;
    env.durable.save(&record_to_row(record))
}

fn closure_refusal(block: ClosureBlock) -> AmError {
    AmError::new(
        ErrorCode::Forbidden,
        format!("request closure refused: {block:?}"),
    )
}

#[cfg(test)]
#[path = "subject_request_service_report_tests.rs"]
mod subject_request_service_report_tests;

#[cfg(test)]
#[path = "subject_request_service_tests.rs"]
mod subject_request_service_tests;

#[cfg(test)]
#[path = "subject_request_service_pg_tests.rs"]
mod subject_request_service_pg_tests;

#[cfg(test)]
#[path = "subject_request_service_facade_tests.rs"]
mod subject_request_service_facade_tests;
