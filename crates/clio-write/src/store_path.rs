// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for span-verify-then-gated-store orchestration.
//!
//! ## Owns
//! - Locked gate order: structural → span verify → taxonomy → five-factor
//! - `evidence_ref` → `source_ref` mapping and required `source_text`
//!
//! ## Does not own
//! - Extractor backends or admission scoring math
//!
//! ## Boundary
//!
//! Changes to verified write wiring should normally require changes here.
//! Changes to match rules should not require changes here.

use crate::schema::FieldSpec;
use crate::telemetry::{VerifyEvent, VerifySink, redact_preview};
use crate::verify::{VerifyConfig, VerifyReport, verify_snapshot};
use clio_admission::decision::{Candidate, Decision, FactorBreakdown};
use clio_admission::gated::{GateRequest, SignalSource, gated_create};
use clio_admission::{AdmissionEvent, AdmissionPolicy, EventSink, admit_preview, signals_for};
use clio_types::{AmError, ErrorCode, MemoryItem, OpContext};

/// Inputs for a snapshot-aware gated store (item is owned so provenance can be filled).
#[derive(Debug)]
pub struct VerifiedStoreRequest<'a> {
    /// Candidate item (snapshot optional).
    pub item: MemoryItem,
    /// Bank / actor context.
    pub ctx: &'a OpContext,
    /// Inline source text required when `item.snapshot` is present.
    pub source_text: Option<&'a str>,
    /// Store-tool alias; copied into `item.source_ref` when that field is empty.
    pub evidence_ref: Option<&'a str>,
    /// Extractive field specs for the snapshot schema.
    pub fields: &'a [FieldSpec],
    /// Span-verify knobs.
    pub verify: VerifyConfig,
    /// Thresholds / weights for admission.
    pub policy: AdmissionPolicy<'a>,
    /// Admission accept/reject sink.
    pub admit_sink: &'a dyn EventSink,
    /// Verify telemetry sink.
    pub verify_sink: &'a dyn VerifySink,
    /// Event timestamp.
    pub timestamp: &'a str,
}

/// Map `evidence_ref` into `item.source_ref` when provenance is unset.
pub fn apply_evidence_ref(item: &mut MemoryItem, evidence_ref: Option<&str>) {
    if item.source_ref.as_ref().is_none_or(String::is_empty)
        && let Some(r) = evidence_ref.filter(|s| !s.is_empty())
    {
        item.source_ref = Some(r.to_owned());
    }
}

fn verify_reject(report: &VerifyReport, profile_version: u32) -> Decision {
    let reason = report
        .failures
        .iter()
        .map(|f| format!("{}: {}", f.field, f.reason))
        .collect::<Vec<_>>()
        .join("; ");
    Decision {
        pass: false,
        admission_score: 0.0,
        factors: FactorBreakdown {
            utility: 0.0,
            confidence: 0.0,
            novelty: 0.0,
            type_prior: 0.0,
        },
        rejection_reason: Some(format!("span_verify: {reason}")),
        profile_version,
        mission_rule_ids: Vec::new(),
        mission_rule_labels: Vec::new(),
    }
}

fn emit_verify(
    sink: &dyn VerifySink,
    operation: &str,
    item: &MemoryItem,
    ctx: &OpContext,
    timestamp: &str,
    report: &VerifyReport,
) {
    sink.emit(VerifyEvent {
        operation: operation.into(),
        bank: ctx.bank.as_str().into(),
        actor: ctx.actor.as_str().into(),
        principal: ctx.principal().map(|label| label.as_str().to_owned()),
        source_ref: item.source_ref.clone().unwrap_or_default(),
        timestamp: timestamp.into(),
        failure_fields: report.failures.iter().map(|f| f.field.clone()).collect(),
        preview: redact_preview(item.snapshot.as_ref(), item.gist.as_deref(), None),
    });
}

fn require_source_text<'a>(
    item: &MemoryItem,
    source_text: Option<&'a str>,
) -> Result<Option<&'a str>, AmError> {
    if item.snapshot.is_none() {
        return Ok(None);
    }
    source_text
        .filter(|s| !s.is_empty())
        .map(Some)
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::InvalidArgument,
                "source_text is required when snapshot is present",
            )
        })
}

/// Preview: structural validate + optional span verify + admission score (no write).
///
/// # Errors
/// Missing `source_text` when snapshot present; structural / signal errors.
#[allow(clippy::too_many_arguments)]
pub fn admit_preview_verified(
    item: &mut MemoryItem,
    ctx: &OpContext,
    source_text: Option<&str>,
    evidence_ref: Option<&str>,
    fields: &[FieldSpec],
    verify: &VerifyConfig,
    candidate: Candidate,
    source: &dyn SignalSource,
    policy: &AdmissionPolicy<'_>,
    verify_sink: &dyn VerifySink,
    timestamp: &str,
) -> Result<Decision, AmError> {
    apply_evidence_ref(item, evidence_ref);
    item.validate()?;
    if let Some(text) = require_source_text(item, source_text)? {
        let report = verify_snapshot(
            text,
            item.snapshot.as_ref().expect("checked"),
            fields,
            verify,
        );
        if !report.ok {
            emit_verify(verify_sink, "verify_fail", item, ctx, timestamp, &report);
            return Ok(verify_reject(&report, policy.profile_version()));
        }
        emit_verify(verify_sink, "verify_ok", item, ctx, timestamp, &report);
    }
    let signals = signals_for(item, source)?;
    admit_preview(candidate, &signals, policy)
}

/// Gated store with locked order: validate → span verify → taxonomy/score → write.
///
/// # Errors
/// Missing `source_text` when snapshot present; structural / factor / write errors.
pub fn gated_store_verified(
    req: &mut VerifiedStoreRequest<'_>,
    candidate: Candidate,
    source: &dyn SignalSource,
    write: &mut dyn FnMut(&MemoryItem) -> Result<(), AmError>,
) -> Result<Decision, AmError> {
    apply_evidence_ref(&mut req.item, req.evidence_ref);
    if req.ctx.bank.as_str().is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    req.item.validate()?;
    if let Some(text) = require_source_text(&req.item, req.source_text)? {
        let report = verify_snapshot(
            text,
            req.item.snapshot.as_ref().expect("checked"),
            req.fields,
            &req.verify,
        );
        if !report.ok {
            emit_verify(
                req.verify_sink,
                "verify_fail",
                &req.item,
                req.ctx,
                req.timestamp,
                &report,
            );
            let decision = verify_reject(&report, req.policy.profile_version());
            req.admit_sink.emit(AdmissionEvent {
                operation: "admit_reject".into(),
                candidate_key: candidate.key().into(),
                candidate_kind: candidate.kind().as_str().into(),
                admission_score: Some(0.0),
                rejection_reason: decision.rejection_reason.clone(),
                bank: req.ctx.bank.as_str().into(),
                actor: req.ctx.actor.as_str().into(),
                timestamp: req.timestamp.into(),
                preview: redact_preview(req.item.snapshot.as_ref(), req.item.gist.as_deref(), None),
            });
            return Ok(decision);
        }
        emit_verify(
            req.verify_sink,
            "verify_ok",
            &req.item,
            req.ctx,
            req.timestamp,
            &report,
        );
    }
    let gate = GateRequest {
        item: &req.item,
        ctx: req.ctx,
        policy: req.policy.clone(),
        sink: req.admit_sink,
        timestamp: req.timestamp,
    };
    gated_create(&gate, candidate, source, write)
}

#[cfg(test)]
#[path = "store_path_test_support.rs"]
mod store_path_test_support;

#[cfg(test)]
#[path = "store_path_tests.rs"]
mod store_path_tests;

#[cfg(test)]
#[path = "store_path_principal_tests.rs"]
mod store_path_principal_tests;

#[cfg(test)]
#[path = "store_path_scope_tests.rs"]
mod store_path_scope_tests;
