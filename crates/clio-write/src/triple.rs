// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for gated bi-temporal triple tools and item invalidation.
//!
//! ## Owns
//! - `triple_add` / `triple_query` / `triple_end` / `invalidate` orchestration
//! - Discrete-path `update_rule` guard and mutator telemetry
//!
//! ## Does not own
//! - SQL drivers (see `clio-store`) or admission scoring math
//!
//! ## Boundary
//!
//! Changes to triple tool wiring should normally require changes here.
//! Changes to interval algebra should not require changes here.

use clio_admission::decision::{Candidate, Decision};
use clio_admission::gated::{GateRequest, SignalSource, gated_create};
use clio_admission::{AdmissionPolicy, EventSink};
use clio_store::Store;
use clio_types::{
    AmError, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext, SourceType,
    TimeAxis, TripleAddOutcome, TripleEndOutcome, TripleQuery, TripleRecord, UpdateRule,
    ensure_discrete_update_rule,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Monotonic suffix so two triples sharing one caller timestamp cannot collide.
static TRIPLE_ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// Collision-resistant id: `{prefix}-{timestamp}-{nanos}-{seq}-{pid}`.
fn unique_id(prefix: &str, timestamp: &str) -> String {
    let seq = TRIPLE_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{prefix}-{timestamp}-{nanos}-{seq}-{}", std::process::id())
}

/// Telemetry for triple/invalidate mutators (spirit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TripleEvent {
    /// `triple_add` | `triple_end` | `invalidate`.
    pub operation: String,
    /// Bank isolation key.
    pub bank: String,
    /// Attribution actor.
    pub actor: String,
    /// Caller timestamp.
    pub timestamp: String,
    /// New or closed edge/item id.
    pub edge_or_item_id: String,
    /// Prior edge ids closed by supersession.
    pub prior_ids: Vec<String>,
    /// Optional replacement linkage.
    pub replacement_id: Option<String>,
}

/// Sink for triple mutator telemetry.
pub trait TripleSink: std::fmt::Debug {
    /// Record one event.
    fn emit(&self, event: TripleEvent);
}

/// In-memory sink for tests.
#[derive(Debug, Default)]
pub struct TripleVecSink {
    events: Mutex<Vec<TripleEvent>>,
}

impl TripleVecSink {
    /// Snapshot of recorded events.
    #[must_use]
    pub fn events(&self) -> Vec<TripleEvent> {
        self.events.lock().expect("triple sink poisoned").clone()
    }
}

impl TripleSink for TripleVecSink {
    fn emit(&self, event: TripleEvent) {
        self.events
            .lock()
            .expect("triple sink poisoned")
            .push(event);
    }
}

/// Inputs for gated `triple_add`.
#[derive(Debug)]
pub struct TripleAddArgs<'a> {
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// SPO subject.
    pub subject: &'a str,
    /// SPO predicate.
    pub predicate: &'a str,
    /// SPO object payload.
    pub object: &'a str,
    /// Fact or belief.
    pub epistemic_kind: EpistemicKind,
    /// Required iff belief.
    pub source_type: Option<SourceType>,
    /// Optional confidence.
    pub confidence: Option<f64>,
    /// Valid-time start (defaults to `now`).
    pub valid_from: Option<&'a str>,
    /// Optional valid-time end on the new edge.
    pub valid_until: Option<&'a str>,
    /// Close prior open subject+predicate (default true).
    pub supersede: bool,
    /// Optional snapshot item ref.
    pub snapshot_ref: Option<&'a str>,
    /// Optional gist item ref.
    pub gist_ref: Option<&'a str>,
    /// Optional bounded descriptive context, validated with
    /// `clio_types::validate_context` and sealed inside the carrier item's
    /// dual-content body. Not stored as a triple column.
    pub context: Option<&'a str>,
    /// Reject continuous attempts when set.
    pub update_rule: Option<UpdateRule>,
    /// Data-subject id for DEK.
    pub subject_id: &'a str,
    /// Admission policy.
    pub policy: AdmissionPolicy<'a>,
    /// Admission sink.
    pub admit_sink: &'a dyn EventSink,
    /// Triple mutator sink.
    pub triple_sink: &'a dyn TripleSink,
    /// Event / transaction timestamp (also default valid_from).
    pub timestamp: &'a str,
    /// Score + validate without persisting (batch pre-flight mode).
    pub dry_run: bool,
}

/// Full result of one gated `triple_add` (decision + commit + dry-run payload).
#[derive(Debug)]
pub struct TripleAddReturn {
    /// Admission decision (pass/score/rejection_reason).
    pub decision: Decision,
    /// Committed outcome (absent when rejected or `dry_run`).
    pub outcome: Option<TripleAddOutcome>,
    /// Would-write payload for the atomic batch executor (staged only on
    /// `dry_run` with a passing decision; never persisted).
    pub payload: Option<TripleBatchPayload>,
}

/// Would-write payload for a dry-run triple add (batch executor input).
#[derive(Debug, Clone)]
pub struct TripleBatchPayload {
    /// Admitted carrier item (score stamped).
    pub item: MemoryItem,
    /// SPO edge referencing the carrier item id.
    pub edge: TripleRecord,
    /// Close prior open subject+predicate edges.
    pub supersede: bool,
}

/// Gated `triple_add`: episodic admission then atomic supersede+insert.
///
/// # Errors
/// Wrong update rule, admission failure, or store errors.
pub fn triple_add(
    store: &dyn Store,
    signals: &dyn SignalSource,
    args: &TripleAddArgs<'_>,
) -> Result<TripleAddReturn, AmError> {
    ensure_discrete_update_rule(args.update_rule)?;
    if args.ctx.bank.as_str().is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    // Fail closed before any write (including dry-run staging): an invalid
    // context must never reach admission or the store.
    if let Some(context) = args.context {
        clio_types::validate_context(context)?;
    }
    let valid_from = args.valid_from.unwrap_or(args.timestamp);
    let edge_id = unique_id("trp", args.timestamp);
    let item_id = unique_id("itm", args.timestamp);
    let item = triple_carrier(args, item_id.clone(), valid_from);
    let edge = TripleRecord {
        id: edge_id,
        bank_id: args.ctx.bank.as_str().into(),
        item_id,
        subject: args.subject.into(),
        predicate: args.predicate.into(),
        object: args.object.into(),
        epistemic_kind: args.epistemic_kind,
        source_type: args.source_type,
        confidence: args.confidence,
        valid_from: valid_from.into(),
        valid_until: args.valid_until.map(str::to_owned),
        tx_from: args.timestamp.into(),
        tx_until: None,
        snapshot_ref: args.snapshot_ref.map(str::to_owned),
        gist_ref: args.gist_ref.map(str::to_owned),
        context: args.context.map(str::to_owned),
        created_at: args.timestamp.into(),
        updated_at: None,
    };
    let mut outcome = None;
    let mut payload = None;
    let mut write = |admitted: &MemoryItem| {
        let mut e = edge.clone();
        e.item_id.clone_from(&admitted.id);
        if args.dry_run {
            payload = Some(TripleBatchPayload {
                item: admitted.clone(),
                edge: e,
                supersede: args.supersede,
            });
            return Ok(());
        }
        let o = store.commit_triple_add(admitted, &e, args.supersede, args.ctx.actor.as_str())?;
        outcome = Some(o);
        Ok(())
    };
    let req = GateRequest {
        item: &item,
        ctx: args.ctx,
        policy: args.policy.clone(),
        sink: args.admit_sink,
        timestamp: args.timestamp,
    };
    let decision = gated_create(
        &req,
        Candidate::Episodic(EpisodicType::Triple),
        signals,
        &mut write,
    )?;
    if let Some(ref o) = outcome {
        args.triple_sink.emit(TripleEvent {
            operation: "triple_add".into(),
            bank: args.ctx.bank.as_str().into(),
            actor: args.ctx.actor.as_str().into(),
            timestamp: args.timestamp.into(),
            edge_or_item_id: o.new_edge_id.clone(),
            prior_ids: o.prior_edge_ids.clone(),
            replacement_id: None,
        });
    }
    Ok(TripleAddReturn {
        decision,
        outcome,
        payload,
    })
}

/// Build the episodic carrier item that holds the triple's snapshot/gist halves.
fn triple_carrier(args: &TripleAddArgs<'_>, item_id: String, valid_from: &str) -> MemoryItem {
    MemoryItem {
        id: item_id,
        bank_id: args.ctx.bank.as_str().into(),
        subject_id: args.subject_id.into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Triple),
        epistemic_kind: args.epistemic_kind,
        source_type: args.source_type,
        confidence: args.confidence,
        admission_score: None,
        source_ref: None,
        context: args.context.map(str::to_owned),
        snapshot: Some(json!({
            "subject": args.subject,
            "predicate": args.predicate,
            "object": args.object,
        })),
        gist: Some(format!(
            "{} {} {}",
            args.subject, args.predicate, args.object
        )),
        valid_from: valid_from.into(),
        valid_until: args.valid_until.map(str::to_owned),
        tx_from: args.timestamp.into(),
        tx_until: None,
        created_at: args.timestamp.into(),
        updated_at: args.timestamp.into(),
    }
}

/// Point-in-time / pattern `triple_query`.
///
/// # Errors
/// Store or validation failures.
pub fn triple_query(store: &dyn Store, query: &TripleQuery) -> Result<Vec<TripleRecord>, AmError> {
    store.query_triples(query)
}

/// Expire open SPO edges without replacement.
///
/// # Errors
/// Wrong update rule or store failures.
#[allow(clippy::too_many_arguments)]
pub fn triple_end(
    store: &dyn Store,
    ctx: &OpContext,
    subject: &str,
    predicate: &str,
    object: Option<&str>,
    valid_until: &str,
    timestamp: &str,
    update_rule: Option<UpdateRule>,
    sink: &dyn TripleSink,
) -> Result<TripleEndOutcome, AmError> {
    ensure_discrete_update_rule(update_rule)?;
    let out = store.triple_end(
        ctx.bank.as_str(),
        subject,
        predicate,
        object,
        valid_until,
        timestamp,
    )?;
    sink.emit(TripleEvent {
        operation: "triple_end".into(),
        bank: ctx.bank.as_str().into(),
        actor: ctx.actor.as_str().into(),
        timestamp: timestamp.into(),
        edge_or_item_id: out.closed_ids.first().cloned().unwrap_or_default(),
        prior_ids: out.closed_ids.clone(),
        replacement_id: None,
    });
    Ok(out)
}

/// Invalidate an item-shaped discrete record (not an SPO edge id).
///
/// # Errors
/// Wrong update rule, triple-id misuse, or store failures.
pub fn invalidate(
    store: &dyn Store,
    ctx: &OpContext,
    item_id: &str,
    replacement_id: Option<&str>,
    timestamp: &str,
    update_rule: Option<UpdateRule>,
    sink: &dyn TripleSink,
) -> Result<TripleEndOutcome, AmError> {
    ensure_discrete_update_rule(update_rule)?;
    let out = store.invalidate_item(
        ctx.bank.as_str(),
        item_id,
        timestamp,
        timestamp,
        replacement_id,
        ctx.actor.as_str(),
    )?;
    sink.emit(TripleEvent {
        operation: "invalidate".into(),
        bank: ctx.bank.as_str().into(),
        actor: ctx.actor.as_str().into(),
        timestamp: timestamp.into(),
        edge_or_item_id: item_id.into(),
        prior_ids: out.closed_ids.clone(),
        replacement_id: replacement_id.map(str::to_owned),
    });
    Ok(out)
}

/// Re-export axis default for callers.
#[must_use]
pub fn default_time_axis() -> TimeAxis {
    TimeAxis::Valid
}

#[cfg(test)]
#[path = "triple_tests.rs"]
mod triple_tests;
