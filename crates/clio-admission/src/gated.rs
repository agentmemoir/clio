// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for the gated public write wrapper and reject telemetry.
//!
//! ## Owns
//! - `GateRequest`, `gated_create`, and raw-discriminator `gated_create_raw`
//! - Accept/reject events with secret-masked previews
//!
//! ## Does not own
//! - Scoring decisions (see `decision`)
//! - Durable storage drivers
//!
//! ## Boundary
//!
//! Changes to write wiring or event shape should normally require changes here.
//! Changes to scoring math should not require changes here.
//!
//! Keep this module focused on gated writes and telemetry.

use crate::decision::{Candidate, Decision, SignalInputs, decide, gate_reject};
use crate::policy::AdmissionPolicy;
use clio_config::secret::{masked_clone, scrub_inline_secrets};
use clio_types::{AmError, EpisodicType, ErrorCode, MemoryItem, OpContext, SemanticCategory};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;

/// Durable-shaped admit/reject event for the telemetry sink.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdmissionEvent {
    /// `admit_accept` or `admit_reject`.
    pub operation: String,
    /// Category or episodic-type key.
    pub candidate_key: String,
    /// `semantic` or `episodic`.
    pub candidate_kind: String,
    /// Score when scoring completed.
    pub admission_score: Option<f64>,
    /// Why a reject happened.
    pub rejection_reason: Option<String>,
    /// Isolation key.
    pub bank: String,
    /// Attribution actor.
    pub actor: String,
    /// Caller-supplied timestamp.
    pub timestamp: String,
    /// Secret-masked snapshot preview.
    pub preview: Value,
}

/// Telemetry sink for admit/reject events.
pub trait EventSink: std::fmt::Debug {
    /// Record one admission event.
    fn emit(&self, event: AdmissionEvent);
}

/// In-memory sink for tests and single-process hosts.
#[derive(Debug, Default)]
pub struct VecSink {
    events: Mutex<Vec<AdmissionEvent>>,
}

impl VecSink {
    /// Read all recorded events.
    #[must_use]
    pub fn events(&self) -> Vec<AdmissionEvent> {
        self.events.lock().expect("admission sink poisoned").clone()
    }
}

impl EventSink for VecSink {
    fn emit(&self, event: AdmissionEvent) {
        self.events
            .lock()
            .expect("admission sink poisoned")
            .push(event);
    }
}

/// Masked preview of an item for reject events.
fn masked_preview(item: &MemoryItem) -> Value {
    let snapshot = item
        .snapshot
        .clone()
        .map_or(Value::Null, |s| masked_clone(&s));
    let gist = item.gist.as_deref().map(|g| {
        let mut short = g.chars().take(160).collect::<String>();
        if g.chars().count() > 160 {
            short.push('…');
        }
        scrub_inline_secrets(&short)
    });
    serde_json::json!({ "snapshot": snapshot, "gist": gist })
}

fn emit_decision(
    sink: &dyn EventSink,
    operation: &str,
    candidate: Candidate,
    item: &MemoryItem,
    ctx: &OpContext,
    timestamp: &str,
    decision: &Decision,
) {
    sink.emit(AdmissionEvent {
        operation: operation.into(),
        candidate_key: candidate.key().into(),
        candidate_kind: candidate.kind().as_str().into(),
        admission_score: Some(decision.admission_score),
        rejection_reason: decision.rejection_reason.clone(),
        bank: ctx.bank.as_str().into(),
        actor: ctx.actor.as_str().into(),
        timestamp: timestamp.into(),
        preview: masked_preview(item),
    });
}

/// Neighbor/prior fetcher for the gated path (injected; not owned by this crate).
///
/// `clio-admission` is pure scoring logic and ships **no** default store adapter.
/// Hosts (tests, `clio-write`, MCP) must supply a `SignalSource`. Empty neighbor
/// lists are only correct when the bank sample is empty — otherwise novelty
/// scores `1.0` and can inflate admission. Store down → `factor_unavailable`.
///
/// Scoring text and provenance always come from the item itself (see
/// [`candidate_text`]), so a decision can never detach from the bytes being
/// persisted.
pub trait SignalSource: std::fmt::Debug {
    /// Neighbor texts for the novelty signal.
    ///
    /// # Errors
    /// `factor_unavailable` when the store backend is down.
    fn neighbor_texts(&self, item: &MemoryItem) -> Result<Vec<String>, AmError>;

    /// Age of the nearest prior in days; `None` means no prior exists.
    ///
    /// # Errors
    /// `factor_unavailable` when the store backend is down.
    fn prior_age_days(&self, item: &MemoryItem) -> Result<Option<f64>, AmError>;

    /// Neighbor texts and prior age from one lookup.
    ///
    /// Hosts whose lookup is expensive (a store scan) SHOULD override this so a
    /// single scan serves both the novelty and recency factors. The default
    /// calls the two accessors above, so existing sources keep their behavior.
    ///
    /// # Errors
    /// `factor_unavailable` when the store backend is down.
    fn neighbor_signals(&self, item: &MemoryItem) -> Result<(Vec<String>, Option<f64>), AmError> {
        Ok((self.neighbor_texts(item)?, self.prior_age_days(item)?))
    }
}

/// Candidate text under test: gist plus snapshot, in that order.
///
/// This is the single source of scoring text on the gated path; callers must
/// not substitute other text for it.
#[must_use]
pub fn candidate_text(item: &MemoryItem) -> String {
    let gist = item.gist.as_deref().unwrap_or("");
    match &item.snapshot {
        Some(snapshot) => format!("{gist}\n{snapshot}"),
        None => gist.to_owned(),
    }
}

/// Build scoring inputs for an item from its own content plus store lookups.
///
/// # Errors
/// `factor_unavailable` when a store lookup fails.
pub fn signals_for(item: &MemoryItem, source: &dyn SignalSource) -> Result<SignalInputs, AmError> {
    let (neighbors, prior_age_days) = source
        .neighbor_signals(item)
        .map_err(as_factor_unavailable)?;
    Ok(SignalInputs {
        text: candidate_text(item),
        source_type: item.source_type,
        neighbors: Some(neighbors),
        prior_age_days,
        recency_unavailable: false,
    })
}

/// Keep an existing `factor_unavailable`, else wrap the backend failure.
fn as_factor_unavailable(e: AmError) -> AmError {
    if e.code == ErrorCode::FactorUnavailable {
        return e;
    }
    crate::factors::factor_unavailable(&e.message)
}

/// Shared inputs for a gated write: item, signals, attribution, and policy.
#[derive(Debug)]
pub struct GateRequest<'a> {
    /// Item proposed for admission.
    pub item: &'a MemoryItem,
    /// Bank isolation and actor attribution.
    pub ctx: &'a OpContext,
    /// Thresholds, priors, weights, and recency `τ`.
    pub policy: AdmissionPolicy<'a>,
    /// Telemetry sink for accept/reject events.
    pub sink: &'a dyn EventSink,
    /// Caller-supplied timestamp for events.
    pub timestamp: &'a str,
}

/// Gated public write: both gates, then the repository create exactly once.
///
/// Rejects never touch `write` but are emitted to `sink`. Backend failures
/// return `factor_unavailable` with no write and no reject event.
///
/// # Errors
/// Invalid bank/item/candidate pairing, unavailable factor backends, or
/// storage failures from `write`.
pub fn gated_create(
    req: &GateRequest<'_>,
    candidate: Candidate,
    source: &dyn SignalSource,
    write: &mut dyn FnMut(&MemoryItem) -> Result<(), AmError>,
) -> Result<Decision, AmError> {
    if req.ctx.bank.as_str().is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if req.timestamp.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "timestamp is required",
        ));
    }
    req.item.validate()?;
    if req.item.kind != candidate.kind() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "candidate kind must match item kind (no cross-kind admission)",
        ));
    }
    match candidate {
        Candidate::Semantic(category) => {
            if req.item.category != Some(category) {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "candidate category must match item category (no threshold shopping)",
                ));
            }
        }
        Candidate::Episodic(tag) => {
            if req.item.episodic_type != Some(tag) {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "candidate episodic_type must match item episodic_type (no threshold shopping)",
                ));
            }
        }
    }
    let signals = signals_for(req.item, source)?;
    let decision = decide(candidate, &signals, &req.policy)?;
    if decision.pass {
        let mut admitted = req.item.clone();
        admitted.admission_score = Some(decision.admission_score);
        write(&admitted)?;
        emit_decision(
            req.sink,
            "admit_accept",
            candidate,
            &admitted,
            req.ctx,
            req.timestamp,
            &decision,
        );
    } else {
        emit_decision(
            req.sink,
            "admit_reject",
            candidate,
            req.item,
            req.ctx,
            req.timestamp,
            &decision,
        );
    }
    Ok(decision)
}

/// Gated write from raw discriminator strings; unknown categories or types
/// become logged rejections without writes.
///
/// # Errors
/// Same as [`gated_create`], plus structurally invalid `kind` values.
pub fn gated_create_raw(
    req: &GateRequest<'_>,
    kind: &str,
    key: &str,
    source: &dyn SignalSource,
    write: &mut dyn FnMut(&MemoryItem) -> Result<(), AmError>,
) -> Result<Decision, AmError> {
    if kind == "semantic" {
        if let Ok(category) = SemanticCategory::parse(key) {
            return gated_create(req, Candidate::Semantic(category), source, write);
        }
        let decision = gate_reject(
            "category",
            key,
            "not in whitelist",
            req.policy.profile_version(),
        );
        emit_gate_reject(
            req.sink,
            kind,
            key,
            req.item,
            req.ctx,
            req.timestamp,
            &decision,
        );
        return Ok(decision);
    }
    if kind == "episodic" {
        if let Ok(tag) = EpisodicType::parse(key) {
            return gated_create(req, Candidate::Episodic(tag), source, write);
        }
        let decision = gate_reject(
            "episodic_type",
            key,
            "not in allowlist",
            req.policy.profile_version(),
        );
        emit_gate_reject(
            req.sink,
            kind,
            key,
            req.item,
            req.ctx,
            req.timestamp,
            &decision,
        );
        return Ok(decision);
    }
    Err(AmError::new(
        ErrorCode::InvalidArgument,
        format!("kind must be semantic|episodic, got `{kind}`"),
    ))
}

fn emit_gate_reject(
    sink: &dyn EventSink,
    kind: &str,
    key: &str,
    item: &MemoryItem,
    ctx: &OpContext,
    timestamp: &str,
    decision: &Decision,
) {
    sink.emit(AdmissionEvent {
        operation: "admit_reject".into(),
        candidate_key: key.into(),
        candidate_kind: kind.into(),
        admission_score: Some(decision.admission_score),
        rejection_reason: decision.rejection_reason.clone(),
        bank: ctx.bank.as_str().into(),
        actor: ctx.actor.as_str().into(),
        timestamp: timestamp.into(),
        preview: masked_preview(item),
    });
}

#[cfg(test)]
#[path = "gated_test_support.rs"]
mod gated_test_support;

#[cfg(test)]
#[path = "gated_tests.rs"]
mod gated_tests;

#[cfg(test)]
#[path = "gated_telemetry_tests.rs"]
mod gated_telemetry_tests;

#[cfg(test)]
#[path = "gated_tag_tests.rs"]
mod gated_tag_tests;
