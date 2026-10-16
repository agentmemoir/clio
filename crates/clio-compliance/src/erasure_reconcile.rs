// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the reconcile a restart runs over pending
//! erasure actions: reloading each pending action's durable record, splitting
//! the refusal classes so an action is never denied when the effect already
//! ran, resuming only the steps still outstanding, and reporting what settled
//! and what stays pending.
//!
//! ## Owns
//! - `reconcile_erasure_actions` and `ErasureReconcileReport`
//! - `ErasureResume` and `ErasureRetryEnv`, the facts a restart supplies
//! - The refusal-class split: an unresolved peer, a hard effect error, or a
//!   record the caller cannot resume stays pending with no outcome, because
//!   the resume facts carry the subject every probe needs; a missing,
//!   undecodable, or column-drifted record settles denied with a no-effect
//!   reason, while a resume whose subject disagrees with a subject-scoped
//!   record refuses with an error before any probe
//!
//! ## Does not own
//! - The flow's retry contract, its step probes, and its settle
//!   (`erasure_retry.rs`)
//! - The erasure workflow (`erase.rs`) and the coordination preflight
//!   (`subject_bank_coordination.rs`)
//!
//! ## Boundary
//!
//! Changes to which pending actions a restart settles, and with what result
//! text, should normally require changes here; changes to the flow's own retry
//! steps should not.
//!
//! Keep this module focused on the restart reconciliation of erasure actions.

use crate::erase::{EraseArgs, EraseOutcome, EraseSink};
use crate::erasure_retry::{
    OUTCOME_RECONCILED, STATUS_COMPLETED, peers_settled, probe_erasure_steps, request_id_of,
    run_erasure_steps, settle_erasure,
};
use crate::subject_bank_coordination::preflight_erase_request;
use crate::subject_request::{RequestScope, SubjectRequestRecord};
use crate::subject_request_events::ProtectedAction;
use crate::subject_request_store_wiring::row_to_record;
use clio_store::{ComplianceStore, DurableRequestStore, LocalEventStore};
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext};
use clio_write::MemTreeMaint;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

/// The per-request facts a restart supplies to resume a pending erasure.
pub struct ErasureResume {
    /// The data-subject id the erasure targets.
    pub subject_id: String,
    /// The bank set the erasure effect fans out to now, read from the effect's
    /// own store just before the call.
    pub live_banks: Vec<String>,
}

/// The deployment facts reconciliation resumes erasures with.
pub struct ErasureRetryEnv<'a> {
    /// Compliance persistence boundary.
    pub store: &'a dyn ComplianceStore,
    /// Owner-only erasure-journal path.
    pub journal_path: &'a Path,
    /// Live `MemTree` engine for the derived refresh (optional).
    pub maint: Option<&'a Arc<MemTreeMaint>>,
    /// Privileged actor the resumed effect runs as.
    pub actor: Actor,
    /// Legal basis recorded for the resumed erasure.
    pub legal_basis: &'a str,
    /// Deployment salt for the one-way hashes.
    pub salt: &'a [u8],
    /// Erasure timestamp (UTC text).
    pub timestamp: &'a str,
    /// Content-free telemetry sink.
    pub sink: &'a dyn EraseSink,
}

/// What one reconciliation pass did with each pending erasure action.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ErasureReconcileReport {
    /// Action ids this pass settled, allowed or denied.
    pub settled: Vec<String>,
    /// Action ids this pass left pending: unfinished work, or an effect whose
    /// evidence this pass cannot observe.
    pub pending: Vec<String>,
}

/// Close the erasure actions a restart left pending, by action id.
///
/// Every pending action of the erasure event kind is reloaded from the durable
/// request store and settled by evidence, never by refusal alone:
///
/// - a record holding a peer that is not acknowledged or disposed stays
///   pending with no outcome written, because a later acknowledgement settles
///   it;
/// - a missing or undecodable record settles denied with a content-free
///   no-effect reason;
/// - a record the caller cannot resume stays pending with no outcome written:
///   the resume facts carry the subject id every probe keys on, so an
///   unresumable restart cannot observe the effect, and a restart must never
///   claim an effect did not happen when it cannot see one;
/// - a resume whose subject disagrees with a subject-scoped record refuses
///   with an error before any probe, so a caller bug cannot drive a probe or
///   a derived refresh over another subject's rows;
/// - the effect probes (key destroyed, committed tombstones) settle a
///   completion and never a denial; when only the derived refresh remains it
///   runs first, so a restart still loses the erased content from the derived
///   state, and a committed purge settles the completion too, because the
///   tombstones prove the erase transaction committed;
/// - a preflight refusal with no effect evidence at all settles denied;
/// - a destroyed key whose purge never committed runs the remaining work
///   through the flow and stays pending until it settles, so a completion is
///   never reported over uncommitted store work and the purge is retried.
///
/// A store probe failure propagates, so an unknown step state settles nothing.
/// An outcome write is at most once because the write is duplicate-tolerant. A
/// pending action of any other event kind is left for its owner.
///
/// # Errors
/// A pending or event read failure, a durable load failure, or a store probe
/// failure.
pub fn reconcile_erasure_actions(
    durable: &dyn DurableRequestStore,
    events: &dyn LocalEventStore,
    env: &ErasureRetryEnv<'_>,
    resume: &dyn Fn(&SubjectRequestRecord) -> Option<ErasureResume>,
) -> Result<ErasureReconcileReport, AmError> {
    let pending = events.pending_actions()?;
    if pending.is_empty() {
        return Ok(ErasureReconcileReport::default());
    }
    let erasure_actions: BTreeSet<String> = events
        .list_events()?
        .into_iter()
        .filter(|event| event.kind == ProtectedAction::Erasure.producer_id())
        .map(|event| event.action_id)
        .collect();
    let mut report = ErasureReconcileReport::default();
    for action_id in pending {
        if !erasure_actions.contains(&action_id) {
            continue;
        }
        match resume_erasure_action(durable, events, env, resume, &action_id)? {
            ResumeOutcome::Settled => report.settled.push(action_id),
            ResumeOutcome::Pending => report.pending.push(action_id),
        }
    }
    Ok(report)
}

/// What one pending action's resume attempt did.
enum ResumeOutcome {
    /// The outcome was settled by this pass, allowed or denied.
    Settled,
    /// The remaining work is unfinished; the action stays pending.
    Pending,
}

/// Reload one pending erasure action and split its refusal classes.
///
/// A record that cannot be loaded or decoded settles denied before any probe,
/// because no restart can observe or resume the effect without it. A record the
/// caller's resume closure refuses stays pending with no outcome written: the
/// resume facts carry the subject id every probe keys on, so an unresumable
/// restart cannot observe the effect, and it must not claim one did not happen.
/// A resume whose subject disagrees with a subject-scoped record is a caller
/// bug and refuses with an error before any probe, so no probe and no effect
/// ever run over the wrong subject.
/// For a record this pass can probe, the effect probes run before any denial,
/// and the peer slots gate completion: a peer that has not acknowledged or been
/// disposed keeps the action pending with no outcome.
/// When only the derived refresh remains, it runs first and the settle follows
/// from the record's slots; a committed purge without the key probe's evidence
/// settles the same completion, because the tombstones prove the erase
/// transaction committed. A destroyed key whose purge never committed runs the
/// remaining work through the flow and reports settled or pending, because an
/// irreversible key loss with failed SQL work must stay pending until the purge
/// commits. Only a preflight refusal with the probes showing no effect at all
/// settles denied. A resumed step's error leaves the action pending, so a hard
/// effect error never becomes a denial.
fn resume_erasure_action(
    durable: &dyn DurableRequestStore,
    events: &dyn LocalEventStore,
    env: &ErasureRetryEnv<'_>,
    resume: &dyn Fn(&SubjectRequestRecord) -> Option<ErasureResume>,
    action_id: &str,
) -> Result<ResumeOutcome, AmError> {
    let Some(request_id) = request_id_of(action_id) else {
        return settle_denied(events, action_id);
    };
    let Some(row) = durable.load(request_id)? else {
        return settle_denied(events, action_id);
    };
    let Ok(record) = row_to_record(&row) else {
        return settle_denied(events, action_id);
    };
    let Some(facts) = resume(&record) else {
        // No resume facts means no subject id to probe with, so this pass
        // cannot observe the effect. Leaving the action pending claims nothing:
        // a denial here would assert a no-effect verdict the code cannot see.
        return Ok(ResumeOutcome::Pending);
    };
    refuse_a_foreign_subject(&record, &facts)?;
    let bank_id = bank_id_of(&record.scope);
    let steps = probe_erasure_steps(env.store, bank_id, &facts.subject_id, request_id)?;
    if !peers_settled(&record) {
        return Ok(ResumeOutcome::Pending);
    }
    let ctx = OpContext::new(Bank::new(bank_id.to_owned()), env.actor);
    let args = EraseArgs {
        store: env.store,
        journal_path: env.journal_path,
        maint: env.maint,
        ctx: &ctx,
        subject_id: &facts.subject_id,
        legal_basis: env.legal_basis,
        request_id,
        reason_code: None,
        confirm: true,
        salt: env.salt,
        timestamp: env.timestamp,
        sink: env.sink,
    };
    if steps.only_refresh_remains() {
        // The whole effect committed: run the derived refresh it still owes,
        // touched by no destructive step, then settle from the record's slots.
        return Ok(settled_or_pending(run_erasure_steps(
            durable, events, &args, action_id, steps,
        )));
    }
    if steps.sql_work_done {
        // The purge committed: this request's tombstones are written inside the
        // erase transaction, so their presence proves it committed and the key
        // destruction rode the same attempt. The live key probe can still read
        // a live key when the restart's KMS no longer remembers the shred, so
        // the committed purge is the durable evidence that settles here, and no
        // destructive step repeats.
        return Ok(if settle_erasure(durable, events, request_id, action_id)? {
            ResumeOutcome::Settled
        } else {
            ResumeOutcome::Pending
        });
    }
    if !steps.key_destruction_done {
        // No purge committed and no key loss: only here may a preflight
        // refusal settle denied, because a denial asserts the effect never ran.
        if preflight_erase_request(&args, &record, &facts.live_banks).is_err() {
            return settle_denied(events, action_id);
        }
    }
    // The key is destroyed and the purge never committed: an irreversible key
    // loss the store work has not caught up with. The remaining work — the
    // purge itself — runs through the flow, and the action stays pending until
    // that work settles: a completion is never settled over uncommitted store
    // work, and the purge stays retryable instead of leaving the pending set.
    Ok(settled_or_pending(run_erasure_steps(
        durable, events, &args, action_id, steps,
    )))
}

/// Refuse a resume whose subject the durable record is not scoped to.
///
/// Only a subject-scoped record can be checked, and the check runs before any
/// probe or effect: the resume subject keys every probe and the derived
/// refresh's detach, so a caller-supplied mismatch would drive work over
/// another subject's surviving identity rows. The refusal is an error, not a
/// denial, because it reports a caller bug and must not close the action.
fn refuse_a_foreign_subject(
    record: &SubjectRequestRecord,
    facts: &ErasureResume,
) -> Result<(), AmError> {
    if let RequestScope::Subject { subject_id, .. } = &record.scope
        && subject_id != &facts.subject_id
    {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "the resume facts name a subject the durable record is not scoped to",
        ));
    }
    Ok(())
}

/// Whether one resumed run settled the action or left work outstanding.
fn settled_or_pending(outcome: Result<EraseOutcome, AmError>) -> ResumeOutcome {
    if outcome.is_ok_and(|outcome| outcome.status == STATUS_COMPLETED) {
        ResumeOutcome::Settled
    } else {
        ResumeOutcome::Pending
    }
}

/// Settle a pending action denied with a content-free no-effect reason.
fn settle_denied(events: &dyn LocalEventStore, action_id: &str) -> Result<ResumeOutcome, AmError> {
    events.record_outcome(action_id, true, OUTCOME_RECONCILED)?;
    Ok(ResumeOutcome::Settled)
}

/// The bank a record's scope names.
fn bank_id_of(scope: &RequestScope) -> &str {
    match scope {
        RequestScope::Bank { bank_id } | RequestScope::Subject { bank_id, .. } => bank_id,
    }
}
