// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the retry contract of a confirmed erasure:
//! one durable intent per erasure action, the durable step probes that decide
//! what a retry still has to run, and the settled outcome the flow writes.
//!
//! ## Owns
//! - `erase_confirmed_request`, the internal request-bound erasure entry
//! - `probe_erasure_steps` and `ErasureStepStatus`, the durable step state
//! - The erasure action id, derived from the request id alone
//! - The settle the flow and reconciliation both write
//!
//! ## Does not own
//! - The reconciliation pass over pending actions (`erasure_reconcile.rs`)
//! - The erasure workflow and its denial-before-key-loss ordering (`erase.rs`)
//! - The coordination preflight (`subject_bank_coordination.rs`)
//! - The durable record mapping and closure facts
//!   (`subject_request_store_wiring.rs`)
//! - The durable event writer and its SQL (`clio-store`)
//!
//! ## Boundary
//!
//! The flow checks the caller's live bank set once, before the intent and any
//! effect, and refuses a changed set; the unguarded window runs from that read
//! to the store's own in-transaction bank re-select. Changes to how a
//! confirmed erasure is retried should normally require changes here; changes
//! to the erasure workflow itself or to the durable event writer should not.
//!
//! The retry entries are internal: no command is bound to them yet, while the
//! crate-root re-export keeps the two entries a command binding must call
//! available. The probe and action-id helpers stay crate-private because no
//! caller outside this crate needs them, and the derived refresh cannot fail
//! by construction (`propagate` returns a count), so the refresh's fault point
//! is the store-side purge fault, not a seam here.
//!
//! Keep this module focused on the confirmed-erasure retry contract.

use crate::erase::{EraseArgs, EraseOutcome, erase_request, propagate};
use crate::subject_bank_coordination::preflight_erase_request;
use crate::subject_request::{PeerState, SubjectRequestRecord};
use crate::subject_request_events::ProtectedAction;
use crate::subject_request_store_wiring::{closure_inputs, row_to_record};
use clio_store::{
    ComplianceStore, DurableRequestStore, LocalEventIntent, LocalEventState, LocalEventStore,
    sha256_hex,
};
use clio_types::{AmError, ErrorCode};

/// The outcome text recorded when the erasure's outstanding work is done.
pub(crate) const OUTCOME_COMPLETED: &str = "erasure completed";
/// The outcome text recorded when reconciliation closes an action without
/// running an effect.
pub(crate) const OUTCOME_RECONCILED: &str = "reconciled without an effect";

/// The status the erasure workflow reports for a complete run.
pub(crate) const STATUS_COMPLETED: &str = "COMPLETED";
/// The status the erasure workflow reports for an incomplete run.
pub(crate) const STATUS_PROCESSING: &str = "PROCESSING";

/// The durable action id one request's erasure is keyed by.
///
/// It is derived from the request id alone, so it is stable across retries
/// and carries no subject content.
#[must_use]
pub(crate) fn erasure_action_id(request_id: &str) -> String {
    format!("{}:{request_id}", ProtectedAction::Erasure.producer_id())
}

/// The request id an erasure action id was derived from, if it has the shape.
pub(crate) fn request_id_of(action_id: &str) -> Option<&str> {
    action_id
        .strip_prefix(ProtectedAction::Erasure.producer_id())
        .and_then(|rest| rest.strip_prefix(':'))
}

/// The durable step state of one confirmed erasure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ErasureStepStatus {
    /// The subject's content key has been destroyed; this step is irreversible.
    pub(crate) key_destruction_done: bool,
    /// The store work committed: this request's erasure tombstones are
    /// present, so the erase transaction that wrote them committed.
    pub(crate) sql_work_done: bool,
}

impl ErasureStepStatus {
    /// True when the destructive work is done and only the derived refresh
    /// remains before the settle.
    #[must_use]
    pub(crate) fn only_refresh_remains(self) -> bool {
        self.key_destruction_done && self.sql_work_done
    }
}

/// Read the durable step state a retry starts from.
///
/// The key probe is the store's negative DEK probe. The store-work probe reads
/// the committed fact: the request's erasure tombstones were written inside
/// the erase transaction, just before it commits, so a rolled-back attempt
/// left none however far it got, and present tombstones mean the purge
/// committed. A probe failure propagates, so an unknown state never skips
/// work.
///
/// # Errors
/// A store probe failure.
pub(crate) fn probe_erasure_steps(
    store: &dyn ComplianceStore,
    bank_id: &str,
    subject_id: &str,
    request_id: &str,
) -> Result<ErasureStepStatus, AmError> {
    Ok(ErasureStepStatus {
        key_destruction_done: store.subject_dek_destroyed(subject_id)?,
        sql_work_done: !store.read_tombstones(bank_id, request_id)?.is_empty(),
    })
}

/// Erase a confirmed request through its durable record, one durable intent,
/// and only the steps that are still pending.
///
/// The record is loaded from the durable store and preflighted before any
/// write; a missing row or a refusal changes nothing. One durable intent is
/// recorded under the request's erasure action id; an event that already
/// exists for that id with this erasure's kind is a resume, so a retry never
/// rewrites the intent, while a same-id row of another kind — or a denied row
/// of this kind, because a denial is terminal — refuses instead of mutating
/// or resuming it. A concurrent caller that won the intent write is
/// resumed too; any other failed intent refuses before the journal denial and
/// before any store effect. The step probes then decide what runs: when the
/// key destruction and the store work are already done, only the derived
/// refresh runs — over the erased subject's surviving identity rows, whose
/// memtree leaves must still be detached — and no destructive work repeats;
/// otherwise the effect runs once. The event settles allowed only when the
/// durable record's current peer and action slots are complete; otherwise it
/// stays pending and the outcome reports processing, with the same status
/// vocabulary the erasure workflow uses. A hard effect error writes no
/// outcome: the action stays pending and the error is returned, so a later
/// resume can still complete it. Denied outcomes are only for reconciliation's
/// own settle, and only when no effect was observed.
///
/// # Errors
/// A missing or undecodable row, a preflight refusal, a failed intent or
/// outcome write, or an erasure workflow failure.
pub fn erase_confirmed_request(
    durable: &dyn DurableRequestStore,
    events: &dyn LocalEventStore,
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
    let record = row_to_record(&row)?;
    preflight_erase_request(args, &record, live_banks)?;
    let action_id = erasure_action_id(request_id);
    ensure_erasure_intent(events, &action_id, args)?;
    let bank_id = args.ctx.bank.as_str();
    let steps = probe_erasure_steps(args.store, bank_id, args.subject_id, args.request_id)?;
    run_erasure_steps(durable, events, args, &action_id, steps)
}

/// Write the action's durable intent unless this action's event already exists.
///
/// An existing event of this erasure's kind is the resume case: the intent is
/// durable, so it is not rewritten. A denied row of this kind refuses instead
/// of resuming, because reconciliation denies only an effect it could not
/// observe and a denial is terminal; the row is never rewritten. A same-id
/// event of another kind refuses, so a foreign action row is never mutated,
/// and a duplicate-intent failure is re-read: a row that now exists with this
/// erasure's kind is a concurrent caller's win and resumes, while anything
/// else returns the original error. Writes use the erasure producer's own
/// vocabulary for the event kind and scope, so reconciliation finds them by
/// kind.
fn ensure_erasure_intent(
    events: &dyn LocalEventStore,
    action_id: &str,
    args: &EraseArgs<'_>,
) -> Result<(), AmError> {
    let producer = ProtectedAction::Erasure.producer_id();
    if let Some(event) = find_action(events, action_id)? {
        if event.kind != producer {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!(
                    "the erasure action id `{action_id}` is held by a `{}` event",
                    event.kind
                ),
            ));
        }
        if event.state == LocalEventState::Denied {
            return Err(AmError::new(
                ErrorCode::Forbidden,
                format!("the erasure action `{action_id}` is denied and is never resumed"),
            ));
        }
        return Ok(());
    }
    let written = events.record_intent(&LocalEventIntent {
        action_id,
        actor: args.ctx.actor.as_str(),
        scope: producer,
        kind: producer,
        event_time: args.timestamp,
    });
    match written {
        Ok(()) => Ok(()),
        Err(err) => {
            let resumed =
                find_action(events, action_id)?.is_some_and(|event| event.kind == producer);
            if resumed { Ok(()) } else { Err(err) }
        }
    }
}

/// The event row held by an action id, if one exists.
fn find_action(
    events: &dyn LocalEventStore,
    action_id: &str,
) -> Result<Option<clio_store::LocalEventView>, AmError> {
    Ok(events
        .list_events()?
        .into_iter()
        .find(|event| event.action_id == action_id))
}

/// Whether every coordinating peer slot of an erasure is settled.
///
/// An acknowledged or disposed peer is settled; a pending peer keeps the
/// erasure's completion pending.
pub(crate) fn peers_settled(record: &SubjectRequestRecord) -> bool {
    record
        .peers
        .values()
        .all(|state| matches!(state, PeerState::Acknowledged | PeerState::Disposed))
}

/// Run the work a confirmed erasure still owes, then settle its event.
///
/// The caller supplies the step probes it already read, so a retry never
/// probes twice. A hard effect error is returned with no outcome written: the
/// workflow reports `PROCESSING` itself when a failed effect lost the key, so
/// an error here means no key loss, and a false denial would misreport an
/// irreversible key loss. Reconciliation reuses this run for the steps a
/// restart resumed.
pub(crate) fn run_erasure_steps(
    durable: &dyn DurableRequestStore,
    events: &dyn LocalEventStore,
    args: &EraseArgs<'_>,
    action_id: &str,
    steps: ErasureStepStatus,
) -> Result<EraseOutcome, AmError> {
    let bank_id = args.ctx.bank.as_str();
    let mut outcome = if steps.only_refresh_remains() {
        // The irreversible work and the store work are done: refresh the
        // derived structures and never repeat a destructive step. The purge
        // keeps the erased subject's identity rows, so they are read back and
        // their memtree leaves detached before the refresh.
        let item_ids: Vec<String> = args
            .store
            .list_subject_items(bank_id, args.subject_id)?
            .into_iter()
            .map(|identity| identity.item_id)
            .collect();
        let refreshed = propagate(args.maint, bank_id, &item_ids);
        completed_outcome(args, refreshed)
    } else {
        erase_request(args)?
    };
    if outcome.status != STATUS_COMPLETED {
        return Ok(outcome);
    }
    if !settle_erasure(durable, events, args.request_id, action_id)? {
        outcome.status = STATUS_PROCESSING.into();
    }
    Ok(outcome)
}

/// The outcome a resumed pass reports when only the derived refresh ran.
///
/// The store work already committed, so its per-step counts are not recovered
/// here and stay zero; the durable record remains the authority on the request
/// itself.
fn completed_outcome(args: &EraseArgs<'_>, memtree_nodes_refreshed: usize) -> EraseOutcome {
    EraseOutcome {
        status: STATUS_COMPLETED.into(),
        request_id: args.request_id.into(),
        subject_hash: sha256_hex(args.salt, args.subject_id),
        shredded_at: args.timestamp.into(),
        item_count: 0,
        tombstones_written: 0,
        vectors_purged: 0,
        assoc_edges_purged: 0,
        memtree_nodes_refreshed,
        triples_purged: 0,
        derived_items_purged: 0,
        persona_context_rows_purged: 0,
    }
}

/// Settle the event when the durable record's current slots allow completion.
///
/// The record is re-read, so a peer slot or a required action recorded while
/// the effect ran keeps the event pending instead of reporting a completion
/// the durable state cannot claim. The action row's own state is read before
/// the write, so a completion is never written over a denied row: a denial is
/// terminal and returns false. An allowed row already holds the completion, so
/// it returns true without a second write. A pending row is completed and
/// returns true. Every step run settles through this, so a completed effect
/// settles the same way however it is reached.
pub(crate) fn settle_erasure(
    durable: &dyn DurableRequestStore,
    events: &dyn LocalEventStore,
    request_id: &str,
    action_id: &str,
) -> Result<bool, AmError> {
    let Some(row) = durable.load(request_id)? else {
        return Ok(false);
    };
    let record = row_to_record(&row)?;
    if !completion_allowed(&record) {
        return Ok(false);
    }
    match find_action(events, action_id)? {
        // A denied row is terminal: no completion may be written over it.
        Some(event) if event.state == LocalEventState::Denied => Ok(false),
        // The allowed outcome was recorded already: nothing to write, and the
        // caller's completion stands.
        Some(event) if event.state == LocalEventState::Allowed => Ok(true),
        // A pending row is the only row the allowed outcome is written to; a
        // missing row is not the intent gate's case and errors below.
        _ => {
            events.record_outcome(action_id, false, OUTCOME_COMPLETED)?;
            Ok(true)
        }
    }
}

/// Whether every coordinating slot of the erasure is settled.
///
/// Only the open-action count of the closure facts is read here; the product
/// path a request names is the request ledger's own completion decision.
fn completion_allowed(record: &SubjectRequestRecord) -> bool {
    peers_settled(record) && closure_inputs(record, "").open_copy_actions == 0
}
