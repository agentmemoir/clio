// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pre-effect subject-bank coordination
//! refusal of a confirmed erasure: every bank sharing the subject key must
//! match the confirmed scope and the peer banks the request recorded before
//! any effect. An unresolved peer slot refuses technical completion in the
//! erasure's completion gate instead, so the pre-effect check keeps only the
//! frozen bank set.
//!
//! ## Owns
//! - `require_all_banks_confirmed`, the pure check over the confirmed record
//!   and the banks that share the subject key now
//! - `preflight_erase_request`, the pre-effect confirmation check a caller
//!   runs before a durable intent and any effect
//! - `coordinated_erase_request`, the erase entry that refuses before
//!   [`crate::erase::erase_request`] runs any effect
//!
//! ## Does not own
//! - The erasure workflow and its authorization gate (`erase.rs`)
//! - The durable request record (`subject_request_frozen.rs`) and the ledger
//!   register/apply contracts (`subject_request.rs`)
//! - Key destruction or intent creation: the check writes nothing
//!
//! ## Boundary
//!
//! Changes to what a coordinated erasure must prove before effects should
//! normally require changes here. Changes to the erasure workflow or the
//! request ledger should not require changes here.
//!
//! Keep this module focused on the subject-bank coordination refusal.

use crate::erase::{EraseArgs, EraseOutcome, erase_request};
use crate::subject_request::{RequestKind, RequestScope, RequestState, SubjectRequestRecord};
use clio_types::{AmError, ErrorCode};
use std::collections::BTreeSet;

/// Refuse when the bank set changed after confirmation.
///
/// The confirmed record's coordination set is its scope bank plus its peer
/// banks, the banks its confirmed peer records covered when it opened. The
/// banks sharing the subject key now must equal that set exactly, so a bank
/// that joined or left after confirmation refuses the erasure and a retry
/// compares against the same stored set. A row from before the peer banks
/// were recorded carries an empty set, so the check then covers the scope
/// bank only and refuses any extra live bank.
///
/// The peer slots themselves do not refuse here: an unacknowledged or
/// disposed peer refuses technical completion in the completion gate instead,
/// so an offline peer cannot block the local effect forever.
///
/// The check reads the frozen scope and writes nothing.
///
/// # Errors
/// Refusal when the bank set changed after confirmation.
pub fn require_all_banks_confirmed(
    confirmed: &SubjectRequestRecord,
    live_banks: &[String],
) -> Result<(), AmError> {
    let expected = coordination_set(confirmed);
    let actual: BTreeSet<&str> = live_banks.iter().map(String::as_str).collect();
    if actual != expected {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "the banks sharing the subject key changed after confirmation",
        ));
    }
    Ok(())
}

/// Refuse an erasure whose confirmation does not match the call, before any
/// intent, denial, or effect.
///
/// The confirmed record must carry the call's request id, be an erasure in
/// progress, and be bound to the bank (and subject, for a subject scope) the
/// call targets. `live_banks` must be the bank set the erasure effect fans
/// out to, read from the effect's own store just before the call; the refusal
/// compares that live set with the confirmed scope, so a stale or separately
/// configured list can never widen or narrow the authorized scope. The
/// coordination check runs last, so an incomplete confirmation or a changed
/// bank set refuses before any intent or key loss.
///
/// The preflight reads and writes nothing, so a caller can run it, then a
/// durable intent, then the effect.
///
/// # Errors
/// A refusal naming the first fact the call does not satisfy.
pub fn preflight_erase_request(
    args: &EraseArgs<'_>,
    confirmed: &SubjectRequestRecord,
    live_banks: &[String],
) -> Result<(), AmError> {
    if confirmed.request_id != args.request_id {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "the confirmed request does not match the erasure request id",
        ));
    }
    if confirmed.kind != RequestKind::Erasure {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "the confirmed request is not an erasure",
        ));
    }
    if confirmed.state != RequestState::InProgress {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "the confirmed request is not in progress, so it authorizes no erasure",
        ));
    }
    if !scope_covers(confirmed, args) {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "the confirmed scope does not cover this erasure's bank or subject",
        ));
    }
    require_all_banks_confirmed(confirmed, live_banks)
}

/// Refuse an erasure whose confirmation does not match the call, then erase.
///
/// Test and tooling seam, not the shipping command-layer path: its callers
/// are tests that hold a confirmed record. The shipping erasure path calls
/// the erasure workflow directly without passing through this seam.
///
/// The preflight runs first, so an incomplete confirmation or a changed bank
/// set refuses before any intent or key loss; then the erasure workflow runs
/// unchanged.
///
/// # Errors
/// A coordination refusal, or any [`erase_request`] failure.
pub fn coordinated_erase_request(
    args: &EraseArgs<'_>,
    confirmed: &SubjectRequestRecord,
    live_banks: &[String],
) -> Result<EraseOutcome, AmError> {
    preflight_erase_request(args, confirmed, live_banks)?;
    erase_request(args)
}

/// The confirmed coordination set: the scope bank plus every recorded peer
/// bank.
fn coordination_set(confirmed: &SubjectRequestRecord) -> BTreeSet<&str> {
    let mut banks: BTreeSet<&str> = confirmed.peer_banks.iter().map(String::as_str).collect();
    match &confirmed.scope {
        RequestScope::Bank { bank_id } | RequestScope::Subject { bank_id, .. } => {
            banks.insert(bank_id.as_str());
        }
    }
    banks
}

/// Whether the confirmed scope names the bank (and subject) the call targets.
fn scope_covers(confirmed: &SubjectRequestRecord, args: &EraseArgs<'_>) -> bool {
    match &confirmed.scope {
        RequestScope::Bank { bank_id } => bank_id == args.ctx.bank.as_str(),
        RequestScope::Subject {
            bank_id,
            subject_id,
        } => bank_id == args.ctx.bank.as_str() && subject_id == args.subject_id,
    }
}

#[cfg(test)]
#[path = "subject_bank_coordination_tests.rs"]
mod subject_bank_coordination_tests;

#[cfg(test)]
#[path = "subject_bank_coordination_pg_tests.rs"]
mod subject_bank_coordination_pg_tests;
