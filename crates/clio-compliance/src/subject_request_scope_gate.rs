// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the scope test the mutating request verbs
//! `complete_path`, `close`, and `apply_command` run: a stored record whose
//! frozen scope bank is not the caller's bank is refused, and the refused
//! attempt is appended to the caller's own audit trail before the refusal
//! returns.
//!
//! ## Owns
//! - `require_scope_bank`: the scope-bank comparison and its refusal
//! - The refused-attempt audit row, its operation label, and its detail
//!
//! ## Does not own
//! - The request verbs that call the gate (`subject_request_service`)
//! - The record, scope, and command vocabulary (`subject_request_frozen`,
//!   `subject_request`)
//! - The audit-event persistence (`clio-store`)
//!
//! ## Boundary
//!
//! Changes to which bank may read or move a request, or to what a refused
//! attempt records, should normally require changes here. Changes to a request
//! verb's own sequencing should not.
//!
//! Keep this module focused on the cross-bank refusal and its evidence.

use crate::subject_request::{RequestScope, SubjectRequestRecord};
use clio_store::Store;
use clio_types::{AmError, ErrorCode, OpContext};
use serde_json::json;

/// Refuse a record whose frozen scope bank is not the caller's bank, so a
/// caller in another bank can neither read nor move the record.
///
/// The refused attempt is appended to the caller's own bank audit trail, named
/// by `operation`, so a cross-bank call is evidence instead of silence. The
/// row goes to the caller's bank and never the record's: the attempt is the
/// caller's act, and a caller must not be able to append rows to a bank it does
/// not belong to.
///
/// # Errors
/// [`ErrorCode::Forbidden`] when the record belongs to another bank, or the
/// audit write's own error when the trail cannot be appended.
pub(crate) fn require_scope_bank(
    audit: &dyn Store,
    record: &SubjectRequestRecord,
    ctx: &OpContext,
    operation: &str,
) -> Result<(), AmError> {
    let scope_bank = match &record.scope {
        RequestScope::Bank { bank_id } | RequestScope::Subject { bank_id, .. } => bank_id,
    };
    if scope_bank == ctx.bank.as_str() {
        return Ok(());
    }
    audit.record_audit_event(
        ctx.bank.as_str(),
        operation,
        Some(record.request_id.as_str()),
        ctx.actor.as_str(),
        &json!({
            "reason": "bank_mismatch",
            "record_bank": scope_bank,
            "caller_bank": ctx.bank.as_str(),
        }),
    )?;
    Err(AmError::new(
        ErrorCode::Forbidden,
        "request belongs to another bank",
    ))
}

#[cfg(test)]
#[path = "subject_request_scope_gate_tests.rs"]
mod subject_request_scope_gate_tests;
