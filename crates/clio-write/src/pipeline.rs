// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for extract → verify → retry-once → refuse/store flow.
//!
//! ## Owns
//! - Hard-capped single extraction retry with optional verifier feedback
//! - Non-authoritative diagnostic / gist-only retention on final refuse
//! - Extract-and-verify without store (for parallel chunk workers)
//!
//! ## Does not own
//! - Span matchers or admission scoring
//!
//! ## Boundary
//!
//! Changes to retry control flow should normally require changes here.

use crate::extract::{ExtractCandidate, ExtractRequest, Extractor};
use crate::schema::FieldSpec;
use crate::store_path::{VerifiedStoreRequest, gated_store_verified};
use crate::telemetry::{VerifyEvent, redact_preview};
use crate::verify::{VerifyConfig, VerifyFailure, VerifyReport, verify_snapshot};
use clio_admission::decision::{Candidate, Decision, FactorBreakdown};
use clio_admission::gated::SignalSource;
use clio_types::{AmError, MemoryItem, validate_context};
use serde_json::Value;

/// Outcome of extract-then-verified-store.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractStoreOutcome {
    /// Admission / verify decision.
    pub decision: Decision,
    /// Non-authoritative retention after refuse (gist and/or diagnostic only).
    pub retained: Option<NonAuthoritativeRetention>,
    /// How many extract calls ran (1 or 2).
    pub extract_calls: u32,
}

/// Extract + verify only (no gated store). Used by parallel chunk workers.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractVerifyOutcome {
    /// Verified candidate when span verify passed.
    pub candidate: Option<ExtractCandidate>,
    /// Non-authoritative retention after refuse.
    pub retained: Option<NonAuthoritativeRetention>,
    /// How many extract calls ran (1 or 2).
    pub extract_calls: u32,
    /// Final verify report.
    pub report: VerifyReport,
}

/// Gist / diagnostic kept for offline review — never an admitted snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct NonAuthoritativeRetention {
    /// Optional prose gist.
    pub gist: Option<String>,
    /// Optional diagnostic candidate JSON (may include failed snapshot).
    pub diagnostic: Option<Value>,
    /// Provenance id.
    pub source_ref: String,
}

/// Bundled inputs for extract / store pipelines.
#[derive(Debug)]
pub struct ExtractStoreArgs<'a> {
    /// Pluggable extractor.
    pub extractor: &'a dyn Extractor,
    /// Source text (also the span haystack).
    pub source_text: &'a str,
    /// Snapshot schema/template for the extractor.
    pub schema_template: Value,
    /// Optional structured tool payload.
    pub tool_payload: Option<Value>,
    /// Extractive field specs.
    pub fields: &'a [FieldSpec],
    /// Span-verify knobs.
    pub verify: &'a VerifyConfig,
    /// Item shell (ids, kind, bank); snapshot/gist filled from extract.
    pub item_base: MemoryItem,
    /// Admission candidate discriminator.
    pub candidate: Candidate,
    /// Novelty/recency signal source.
    pub signal_source: &'a dyn SignalSource,
}

/// Inputs for [`extract_and_verify`] (no item shell / admission).
#[derive(Debug)]
pub struct ExtractVerifyArgs<'a> {
    /// Pluggable extractor.
    pub extractor: &'a dyn Extractor,
    /// Source text (also the span haystack).
    pub source_text: &'a str,
    /// Snapshot schema/template for the extractor.
    pub schema_template: Value,
    /// Optional structured tool payload.
    pub tool_payload: Option<Value>,
    /// Optional untrusted descriptive context about the setting.
    pub context: Option<String>,
    /// Extractive field specs.
    pub fields: &'a [FieldSpec],
    /// Span-verify knobs.
    pub verify: &'a VerifyConfig,
    /// Optional verify telemetry sink + bank/actor labels.
    pub emit: Option<VerifyEmitCtx<'a>>,
}

/// Telemetry context for extract-and-verify.
#[derive(Debug, Clone, Copy)]
pub struct VerifyEmitCtx<'a> {
    /// Verify sink.
    pub sink: &'a dyn crate::telemetry::VerifySink,
    /// Bank id for events.
    pub bank: &'a str,
    /// Actor id for events.
    pub actor: &'a str,
    /// Label of the credential that presented the operation; `None` when none
    /// did, so a local event carries no such key.
    pub principal: Option<&'a str>,
    /// Timestamp label.
    pub timestamp: &'a str,
}

fn feedback(report: &VerifyReport) -> String {
    report
        .failures
        .iter()
        .map(|f| format!("{}: {}", f.field, f.reason))
        .collect::<Vec<_>>()
        .join("; ")
}

fn no_snapshot_report() -> VerifyReport {
    VerifyReport {
        ok: false,
        failures: vec![VerifyFailure {
            field: "*".into(),
            reason: "extractor returned no snapshot".into(),
        }],
        spans: vec![],
    }
}

fn report_for(
    source_text: &str,
    cand: &ExtractCandidate,
    fields: &[FieldSpec],
    verify: &VerifyConfig,
) -> VerifyReport {
    match &cand.snapshot {
        Some(snap) => verify_snapshot(source_text, snap, fields, verify),
        None => no_snapshot_report(),
    }
}

fn refuse_decision(report: &VerifyReport, profile_version: u32) -> Decision {
    Decision {
        pass: false,
        admission_score: 0.0,
        factors: FactorBreakdown {
            utility: 0.0,
            confidence: 0.0,
            novelty: 0.0,
            recency: 0.0,
            type_prior: 0.0,
        },
        rejection_reason: Some(format!("span_verify_refuse: {}", feedback(report))),
        profile_version,
        mission_rule_ids: Vec::new(),
        mission_rule_labels: Vec::new(),
    }
}

fn emit_ctx(
    ctx: Option<VerifyEmitCtx<'_>>,
    operation: &str,
    cand: &ExtractCandidate,
    report: &VerifyReport,
) {
    let Some(ctx) = ctx else {
        return;
    };
    ctx.sink.emit(VerifyEvent {
        operation: operation.into(),
        bank: ctx.bank.into(),
        actor: ctx.actor.into(),
        principal: ctx.principal.map(str::to_owned),
        source_ref: cand.source_ref.clone(),
        timestamp: ctx.timestamp.into(),
        failure_fields: report.failures.iter().map(|f| f.field.clone()).collect(),
        preview: redact_preview(
            cand.snapshot.as_ref(),
            cand.gist.as_deref(),
            cand.diagnostic.as_ref(),
        ),
    });
}

/// Extract → verify → on fail extract once more → verify. Does not store.
///
/// The caller-supplied context is validated here: every public
/// extraction entry point funnels through this constructor, so an empty or
/// oversized context is rejected before any extractor call.
///
/// # Errors
/// Extractor transport / parse errors; invalid source `context`.
pub fn extract_and_verify(args: ExtractVerifyArgs<'_>) -> Result<ExtractVerifyOutcome, AmError> {
    if let Some(context) = &args.context {
        validate_context(context)?;
    }
    let mut req = ExtractRequest {
        source_text: args.source_text.to_owned(),
        tool_payload: args.tool_payload,
        schema_template: args.schema_template,
        verify_feedback: None,
        context: args.context,
    };
    let mut candidate_ex = args.extractor.extract(&req)?;
    let mut calls = 1u32;
    let mut report = report_for(args.source_text, &candidate_ex, args.fields, args.verify);
    if !report.ok {
        emit_ctx(args.emit, "verify_fail", &candidate_ex, &report);
        emit_ctx(args.emit, "verify_retry", &candidate_ex, &report);
        req.verify_feedback = Some(feedback(&report));
        candidate_ex = args.extractor.extract(&req)?;
        calls = 2;
        report = report_for(args.source_text, &candidate_ex, args.fields, args.verify);
    }
    if !report.ok {
        emit_ctx(args.emit, "verify_refuse", &candidate_ex, &report);
        return Ok(ExtractVerifyOutcome {
            candidate: None,
            retained: Some(NonAuthoritativeRetention {
                gist: candidate_ex.gist,
                diagnostic: Some(serde_json::json!({
                    "failed_snapshot": candidate_ex.snapshot,
                    "extractor_diagnostic": candidate_ex.diagnostic,
                    "failures": report.failures,
                })),
                source_ref: candidate_ex.source_ref,
            }),
            extract_calls: calls,
            report,
        });
    }
    emit_ctx(args.emit, "verify_ok", &candidate_ex, &report);
    Ok(ExtractVerifyOutcome {
        candidate: Some(candidate_ex),
        retained: None,
        extract_calls: calls,
        report,
    })
}

/// Extract → verify → on fail extract once more → verify → store or refuse.
///
/// # Errors
/// Extractor / store errors.
pub fn extract_verify_store<'a>(
    args: ExtractStoreArgs<'a>,
    store_req_parts: &mut VerifiedStoreRequest<'a>,
    write: &mut dyn FnMut(&MemoryItem) -> Result<(), AmError>,
) -> Result<ExtractStoreOutcome, AmError> {
    let emit = VerifyEmitCtx {
        sink: store_req_parts.verify_sink,
        bank: store_req_parts.ctx.bank.as_str(),
        actor: store_req_parts.ctx.actor.as_str(),
        principal: store_req_parts
            .ctx
            .principal()
            .map(clio_types::PrincipalLabel::as_str),
        timestamp: store_req_parts.timestamp,
    };
    let verified = extract_and_verify(ExtractVerifyArgs {
        extractor: args.extractor,
        source_text: args.source_text,
        schema_template: args.schema_template,
        tool_payload: args.tool_payload,
        context: args.item_base.context.clone(),
        fields: args.fields,
        verify: args.verify,
        emit: Some(emit),
    })?;
    let Some(candidate_ex) = verified.candidate else {
        return Ok(ExtractStoreOutcome {
            decision: refuse_decision(&verified.report, store_req_parts.policy.profile_version()),
            retained: verified.retained,
            extract_calls: verified.extract_calls,
        });
    };
    let mut item = args.item_base;
    item.snapshot = candidate_ex.snapshot;
    item.gist = candidate_ex.gist;
    item.source_ref = Some(candidate_ex.source_ref);
    store_req_parts.item = item;
    store_req_parts.source_text = Some(args.source_text);
    let decision =
        gated_store_verified(store_req_parts, args.candidate, args.signal_source, write)?;
    Ok(ExtractStoreOutcome {
        decision,
        retained: None,
        extract_calls: verified.extract_calls,
    })
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod pipeline_tests;
