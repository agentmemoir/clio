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
//!   reason, a resume whose subject has no matching committed tombstone stays
//!   pending when the request's purge evidence exists, while a resume whose
//!   subject disagrees with a subject-scoped record refuses with an error
//!   before any probe
//! - The peer-denial arm: a pending per-peer event is settled from its
//!   durable acknowledged or refused row by action id with no effect, so a
//!   retry resumes the same action instead of repeating one
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
use crate::subject_request_events::{ProtectedAction, peer_outcome};
use crate::subject_request_store_wiring::row_to_record;
use clio_store::{
    ComplianceStore, DurableRequestStore, LocalEventStore, LocalEventView, PeerActionStore,
};
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext};
use clio_write::MemTreeMaint;
use std::collections::BTreeMap;
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
    /// Durable per-peer action rows the completion gate reads.
    pub peers: &'a dyn PeerActionStore,
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
/// - a record holding a peer with no durable acknowledged row or recorded
///   deletion evidence stays pending with no outcome written, because a later
///   acknowledgement or disposition settles it;
/// - a missing or undecodable record settles denied with a content-free
///   no-effect reason;
/// - a record the caller cannot resume stays pending with no outcome written:
///   the resume facts carry the subject id every probe keys on, so an
///   unresumable restart cannot observe the effect, and a restart must never
///   claim an effect did not happen when it cannot see one;
/// - a resume whose subject disagrees with a subject-scoped record refuses
///   with an error before any probe, so a caller bug cannot drive a probe or
///   a derived refresh over another subject's rows;
/// - a resume whose committed purge evidence exists but names another subject
///   stays pending with no outcome written, because those tombstones prove a
///   different subject's effect committed and this pass cannot observe one for
///   the subject it resumes;
/// - the effect probes (key destroyed, committed tombstones) settle a
///   completion and never a denial; when only the derived refresh remains it
///   runs first, so a restart still loses the erased content from the derived
///   state, and a committed purge whose tombstones name the resumed subject
///   settles the completion too, because they prove the erase transaction
///   committed;
/// - a preflight refusal with no effect evidence at all settles denied;
/// - a destroyed key whose purge never committed runs the remaining work
///   through the flow and stays pending until it settles, so a completion is
///   never reported over uncommitted store work and the purge is retried.
///
/// A pending peer-denial action is settled from its durable per-peer row by
/// action id and runs no effect: an acknowledged or refused row completes the
/// pending event once, while a pending or delivered row, and an event whose
/// durable row is missing, stay pending for a later pass. The peer's row is
/// never rewritten, so a retry resumes the same action and never repeats a
/// completed delivery or purge.
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
    let actionable: BTreeMap<String, LocalEventView> = events
        .list_events()?
        .into_iter()
        .filter(|event| {
            event.kind == ProtectedAction::Erasure.producer_id()
                || event.kind == ProtectedAction::PeerDenial.producer_id()
        })
        .map(|event| (event.action_id.clone(), event))
        .collect();
    let mut report = ErasureReconcileReport::default();
    for action_id in pending {
        let Some(event) = actionable.get(&action_id) else {
            continue;
        };
        let outcome = if event.kind == ProtectedAction::PeerDenial.producer_id() {
            resume_peer_action(env, events, event)?
        } else {
            resume_erasure_action(durable, events, env, resume, &action_id)?
        };
        match outcome {
            ResumeOutcome::Settled => report.settled.push(action_id),
            ResumeOutcome::Pending => report.pending.push(action_id),
        }
    }
    Ok(report)
}

/// Settle one pending peer-denial action from its durable row, with no effect.
///
/// The event's own durable scope names the request the per-peer rows hang off,
/// and the action id selects the row: the acknowledged or refused row settles
/// the pending event exactly once with a content-free result, while a pending
/// or delivered row stays pending for reconciliation. The row is only read,
/// never rewritten, so a retry resumes the same action and never repeats a
/// completed delivery; an event with no matching durable row stays pending
/// rather than claiming a result this pass cannot see.
fn resume_peer_action(
    env: &ErasureRetryEnv<'_>,
    events: &dyn LocalEventStore,
    event: &LocalEventView,
) -> Result<ResumeOutcome, AmError> {
    let rows = env.peers.load_for_request(&event.scope)?;
    let Some(row) = rows.iter().find(|row| row.action_id == event.action_id) else {
        return Ok(ResumeOutcome::Pending);
    };
    match peer_outcome(row) {
        Some((result, denied)) => {
            events.record_outcome(&event.action_id, denied, result)?;
            Ok(ResumeOutcome::Settled)
        }
        None => Ok(ResumeOutcome::Pending),
    }
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
/// A committed purge whose tombstones name another subject keeps the action
/// pending before any settle or effect, because the purge evidence binds the
/// resume to the subject it actually ran for.
/// When only the derived refresh remains, it runs first and the settle follows
/// from the record's slots; a committed purge whose tombstones name the resumed
/// subject settles the same completion without the key probe's evidence,
/// because the tombstones prove the erase transaction committed. A destroyed
/// key whose purge never committed runs the remaining work through the flow and
/// reports settled or pending, because an irreversible key loss with failed SQL
/// work must stay pending until the purge commits. Only a preflight refusal
/// with the probes showing no effect at all settles denied. A resumed step's
/// error leaves the action pending, so a hard effect error never becomes a
/// denial.
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
    let steps = probe_erasure_steps(env.store, bank_id, &facts.subject_id, request_id, env.salt)?;
    let peer_rows = env.peers.load_for_request(request_id)?;
    if !peers_settled(&record, &peer_rows) {
        return Ok(ResumeOutcome::Pending);
    }
    if steps.sql_work_done && !steps.purge_names_subject {
        // The committed purge names another subject: this pass cannot see work
        // for the subject it resumes, so settling would report a completion for
        // someone else's effect, and running the effect would destroy a subject
        // the committed evidence never touched. Keep the action pending; a
        // resume naming the purged subject still settles it.
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
            durable, events, env.peers, &args, action_id, steps,
        )));
    }
    if steps.sql_work_done {
        // The purge committed: this request's tombstones are written inside the
        // erase transaction, so their presence proves it committed and the key
        // destruction rode the same attempt. The live key probe can still read
        // a live key when the restart's KMS no longer remembers the shred, so
        // the committed purge is the durable evidence that settles here, and no
        // destructive step repeats.
        return Ok(
            if settle_erasure(durable, events, env.peers, request_id, action_id)? {
                ResumeOutcome::Settled
            } else {
                ResumeOutcome::Pending
            },
        );
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
        durable, events, env.peers, &args, action_id, steps,
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
