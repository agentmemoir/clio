// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP binding of the durable subject-request
//! verbs: it decodes the ten request/peer tool arguments, calls the shared
//! `clio_compliance::subject_request_service` facade, and returns the separated
//! request status report as JSON.
//!
//! ## Owns
//! - Argument decoding and result shaping for the ten `subject_request_*` tools
//! - Deriving the deployment peer-enablement fact from the effective config
//! - The request-scope, request-kind, request-command, and peer-disposition
//!   argument parsers
//!
//! ## Does not own
//! - The request sequencing, state machine, and record vocabulary
//!   (`clio-compliance`'s `subject_request_service`)
//! - The durable request, event, peer, and copy stores (`clio-store`)
//! - The write-surface tool routing (see `additive_tools` / `write_tools`)
//!
//! ## Boundary
//!
//! Changes to a request tool's argument or result shape should normally require
//! changes here. Changes to the request state machine or the durable stores
//! should not require changes here.
//!
//! Keep this module focused on the MCP subject-request binding.

use crate::read_tools::opt_usize;
use crate::runtime::{McpState, StderrSinks};
use crate::write_tools::{opt_bool, opt_str, req_str, strict_opt_str};
use clio_compliance::subject_request_service as svc;
use clio_compliance::{
    DEFAULT_REVIEW_LIMIT, EraseArgs, ErasureRetryEnv, JournalHold, PeerEnablement, RequestKind,
    RequestScope, SubjectRequestEnv, VerificationResult, tombstone_salt,
};
use clio_store::{CopyInventoryRow, LocalEventState, PeerActionDisposition};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Route one subject-request tool name to its handler (raw result).
#[inline(never)]
pub(crate) fn dispatch_inner(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    match name {
        "subject_request_start" => start(state, args),
        "subject_request_status" => status(state, args),
        "subject_request_retry" => retry(state, args),
        "subject_request_copy" => copy(state, args),
        "subject_request_exception" => exception(state, args),
        "subject_request_events" => events(state, args),
        "subject_request_close" => close(state, args),
        "subject_request_peer_status" => peer_status(state, args),
        "subject_request_peer_retry" => peer_retry(state, args),
        "subject_request_peer_disposition" => peer_disposition(state, args),
        other => Err(AmError::not_implemented(other)),
    }
}

/// The borrowed store handles one request verb sequences over.
pub(crate) fn env(state: &McpState) -> SubjectRequestEnv<'_> {
    SubjectRequestEnv {
        durable: &*state.durable,
        events: &*state.events,
        peers: &*state.peer_actions,
        copies: &*state.copies,
    }
}

/// `subject_request_start`: open one request (or replay it) and report it.
fn start(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let request_id = req_str(args, "request_id")?;
    let kind = parse_kind(req_str(args, "kind")?)?;
    let scope = request_scope(&ctx, args);
    let verification = parse_verification(strict_opt_str(args, "verification")?)?;
    let deadline = strict_opt_str(args, "deadline")?.map(str::to_owned);
    let (enablement, peers, banks) = deployment_enablement(state)?;
    let peer_refs: Vec<&str> = peers.iter().map(String::as_str).collect();
    let bank_refs: Vec<&str> = banks.iter().map(String::as_str).collect();
    let report = svc::open(
        &env(state),
        &ctx,
        request_id,
        kind,
        &scope,
        enablement,
        &peer_refs,
        &bank_refs,
        verification,
        deadline,
    )?;
    Ok(serde_json::to_value(&report).expect("serialize request report"))
}

/// `subject_request_status`: report one stored request.
fn status(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let report = svc::report(&env(state), &ctx, req_str(args, "request_id")?)?;
    Ok(serde_json::to_value(&report).expect("serialize request report"))
}

/// `subject_request_retry`: retry a confirmed erasure and report it.
fn retry(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let request_id = req_str(args, "request_id")?.to_owned();
    let subject_id = req_str(args, "subject_id")?.to_owned();
    let legal_basis = req_str(args, "legal_basis")?.to_owned();
    let confirm = opt_bool(args, "confirm", false)?;
    if !confirm {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "subject_request_retry requires confirm=true; unconfirmed calls never mutate",
        ));
    }
    let timestamp = state.resolve_timestamp(args);
    let salt = tombstone_salt();
    let journal = JournalHold::resolve_journal_path(state.journal_path.as_deref(), &|key| {
        std::env::var(key).ok()
    })?;
    let e_args = EraseArgs {
        store: &*state.compliance,
        journal_path: &journal,
        maint: Some(&state.maint),
        ctx: &ctx,
        subject_id: &subject_id,
        legal_basis: &legal_basis,
        request_id: &request_id,
        reason_code: opt_str(args, "reason_code"),
        confirm,
        salt: &salt,
        timestamp: &timestamp,
        sink: &StderrSinks,
    };
    let live_banks = state
        .bank_scope
        .erase_bank_set(ctx.bank.as_str(), &subject_id)?;
    let report = svc::retry_erasure(&env(state), &ctx, &request_id, &e_args, &live_banks)?;
    Ok(retry_value(&report))
}

/// Fold one erasure retry report into the flat tool result: the request status
/// fields plus the pending action id and the effect counts the retry moved.
pub(crate) fn retry_value(retry: &svc::RetryReport) -> Value {
    let mut value = serde_json::to_value(&retry.report).expect("serialize request report");
    if let Value::Object(map) = &mut value {
        map.insert("action_id".to_owned(), json!(retry.action_id));
        map.insert("effects_before".to_owned(), json!(retry.effects_before));
        map.insert("effects_after".to_owned(), json!(retry.effects_after));
    }
    value
}

/// `subject_request_copy`: record one copy disposition and report the request.
fn copy(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let row = CopyInventoryRow {
        copy_id: req_str(args, "copy_id")?.to_owned(),
        request_id: req_str(args, "request_id")?.to_owned(),
        custody: req_str(args, "custody")?.to_owned(),
        owner: req_str(args, "owner")?.to_owned(),
        required_action: req_str(args, "required_action")?.to_owned(),
        state: req_str(args, "state")?.to_owned(),
        exception_reason: opt_str(args, "exception_reason").map(str::to_owned),
        updated_at: opt_str(args, "updated_at")
            .map_or_else(|| state.resolve_timestamp(args), str::to_owned),
    };
    let report = svc::record_copy(&env(state), &ctx, &row)?;
    Ok(serde_json::to_value(&report).expect("serialize request report"))
}

/// `subject_request_exception`: record a retention exception and report it.
fn exception(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let report = svc::record_exception(
        &env(state),
        &ctx,
        req_str(args, "request_id")?,
        req_str(args, "reason")?,
    )?;
    Ok(serde_json::to_value(&report).expect("serialize request report"))
}

/// `subject_request_events`: read one content-free page of one request's events.
fn events(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let limit = opt_usize(args, "limit").unwrap_or(DEFAULT_REVIEW_LIMIT);
    let cursor = opt_str(args, "cursor");
    let page = svc::review_events(
        &env(state),
        &ctx,
        req_str(args, "request_id")?,
        limit,
        cursor,
    )?;
    let rows: Vec<Value> = page
        .rows
        .iter()
        .map(|row| {
            json!({
                "seq": row.seq,
                "action_id": row.action_id,
                "state": event_state(row.state),
            })
        })
        .collect();
    Ok(json!({
        "rows": rows,
        "next_cursor": page.next_cursor,
        "has_more": page.has_more,
    }))
}

/// `subject_request_close`: accept closure, finalize, and report the request.
fn close(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let report = svc::close(
        &env(state),
        &ctx,
        req_str(args, "request_id")?,
        req_str(args, "product_path")?,
    )?;
    Ok(serde_json::to_value(&report).expect("serialize request report"))
}

/// `subject_request_peer_status`: the derived peer slots of one request.
fn peer_status(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let peers = svc::peer_report(&env(state), &ctx, req_str(args, "request_id")?)?;
    Ok(serde_json::to_value(&peers).expect("serialize peer states"))
}

/// `subject_request_peer_retry`: reconcile the pending erasure actions a
/// restart left, then report the request. Its argument is the request id; the
/// deployment facts the reconcile resumes with come from the runtime state.
fn peer_retry(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let request_id = req_str(args, "request_id")?.to_owned();
    let timestamp = state.resolve_timestamp(args);
    let salt = tombstone_salt();
    let journal = JournalHold::resolve_journal_path(state.journal_path.as_deref(), &|key| {
        std::env::var(key).ok()
    })?;
    let retry = ErasureRetryEnv {
        store: &*state.compliance,
        peers: &*state.peer_actions,
        journal_path: &journal,
        maint: Some(&state.maint),
        actor: ctx.actor,
        legal_basis: opt_str(args, "legal_basis").unwrap_or("OPERATOR_RETRY"),
        salt: &salt,
        timestamp: &timestamp,
        sink: &StderrSinks,
    };
    let report = svc::peer_retry(&env(state), &ctx, &request_id, &retry)?;
    Ok(serde_json::to_value(&report).expect("serialize request report"))
}

/// `subject_request_peer_disposition`: record one peer disposition and report it.
fn peer_disposition(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let report = svc::peer_disposition(
        &env(state),
        &ctx,
        req_str(args, "request_id")?,
        req_str(args, "peer_id")?,
        parse_disposition(req_str(args, "disposition")?)?,
        opt_str(args, "proof_ref"),
    )?;
    Ok(serde_json::to_value(&report).expect("serialize request report"))
}

/// The immutable scope a request id binds to: a subject when one is named.
fn request_scope(ctx: &clio_types::OpContext, args: &Value) -> RequestScope {
    let bank_id = ctx.bank.as_str().to_owned();
    match opt_str(args, "subject_id") {
        Some(subject_id) => RequestScope::Subject {
            bank_id,
            subject_id: subject_id.to_owned(),
        },
        None => RequestScope::Bank { bank_id },
    }
}

/// Derive the deployment's peer-enablement fact from the effective config.
///
/// The single-host predicate reads the same `/sync/omit` pointer the CLI reads;
/// an omitted, non-boolean, or `true` value is a single-host deployment, so the
/// peer set is not applicable.
pub(crate) fn deployment_enablement(
    state: &McpState,
) -> Result<(PeerEnablement, Vec<String>, BTreeSet<String>), AmError> {
    let effective = state
        .config
        .lock()
        .map_err(|_| AmError::new(ErrorCode::Internal, "config runtime lock is poisoned"))?
        .effective_raw();
    let single_host = effective
        .pointer("/sync/omit")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    svc::derive_peer_enablement(&effective, &|key| std::env::var(key).ok(), single_host)
}

fn parse_kind(raw: &str) -> Result<RequestKind, AmError> {
    match raw {
        "copy" => Ok(RequestKind::Copy),
        "erasure" => Ok(RequestKind::Erasure),
        other => Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("unknown request kind `{other}`"),
        )),
    }
}

fn parse_verification(raw: Option<&str>) -> Result<Option<VerificationResult>, AmError> {
    match raw {
        None => Ok(None),
        Some("verified") => Ok(Some(VerificationResult::Verified)),
        Some("unverified") => Ok(Some(VerificationResult::Unverified)),
        Some(other) => Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("unknown verification result `{other}`"),
        )),
    }
}

fn parse_disposition(raw: &str) -> Result<PeerActionDisposition, AmError> {
    match raw {
        "removal_evidence" => Ok(PeerActionDisposition::RemovalEvidence),
        "retention_approved" => Ok(PeerActionDisposition::RetentionApproved),
        "unknown" => Ok(PeerActionDisposition::Unknown),
        other => Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("unknown peer disposition `{other}`"),
        )),
    }
}

fn event_state(state: LocalEventState) -> &'static str {
    match state {
        LocalEventState::Pending => "pending",
        LocalEventState::Allowed => "allowed",
        LocalEventState::Denied => "denied",
    }
}

#[cfg(test)]
#[path = "subject_request_tools_tests.rs"]
mod subject_request_tools_tests;

#[cfg(test)]
#[path = "subject_request_tools_scenario_tests.rs"]
mod subject_request_tools_scenario_tests;
