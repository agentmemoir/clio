// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP-bound persona and history record
//! write tools: `persona_put_stable`, `persona_observe_preference`,
//! `task_upsert`, and `failure_record`, decoded from MCP args into in-process
//! calls.
//!
//! ## Owns
//! - JSON argument decoding and result shaping for those four tools
//!
//! ## Does not own
//! - Gate or admission math; the in-process domain crates own behavior
//! - Item lifecycle mutators and confirmation handling (see `mutator_tools`)
//!
//! ## Boundary
//!
//! Changes to MCP argument decoding or result shaping for persona/history
//! record writes should normally require changes here. Changes to the
//! admission gate or the record stores should not require changes here.
//!
//! Keep this module focused on MCP persona/history record dispatch.

use crate::runtime::{McpState, StderrSinks, StoreSignals};
use crate::write_tools::{gated_write_policy, opt_bool, opt_context, opt_num, opt_str, req_str};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::sync::Arc;

/// `persona_put_stable`: discrete/categorical stable entry upsert.
#[inline(never)]
pub(crate) fn persona_put_stable(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args)?;
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let context = opt_context(args, "context")?;
    let p_args = clio_persona::PersonaPutStableArgs {
        store: &*state.persona,
        continuous: &*state.continuous,
        ctx: &ctx,
        key: req_str(args, "key")?,
        value: req_str(args, "value")?,
        context: context.as_deref(),
        confidence: opt_num(args, "confidence"),
        source: opt_str(args, "source"),
        evidence_ref: opt_str(args, "evidence_ref"),
        policy: gated_write_policy(state, ctx.bank.as_str(), args)?,
        signal_source: &signals,
        admission_sink: &StderrSinks,
        persona_sink: &StderrSinks,
        budget_tokens: state.budget_tokens,
        timestamp: &ts,
        dry_run: opt_bool(args, "dry_run", false)?,
    };
    match clio_persona::persona_put_stable(&p_args)? {
        clio_persona::PersonaPutStableOutcome::Admitted {
            entry,
            decision,
            dropped_keys,
        } => Ok(json!({
            "outcome": "admitted",
            "entry": serde_json::to_value(&entry).expect("serialize tool payload"),
            "decision": serde_json::to_value(&decision).expect("serialize tool payload"),
            "dropped_keys": dropped_keys
        })),
        clio_persona::PersonaPutStableOutcome::Rejected { decision } => Ok(json!({
            "outcome": "rejected",
            "decision": serde_json::to_value(&decision).expect("serialize tool payload")
        })),
        clio_persona::PersonaPutStableOutcome::DryRun { entry, decision } => Ok(json!({
            "outcome": "dry_run",
            "entry": serde_json::to_value(&entry).expect("serialize tool payload"),
            "decision": serde_json::to_value(&decision).expect("serialize tool payload"),
            "dry_run": true
        })),
    }
}

/// `persona_observe_preference`: continuous EMA preference observation.
#[inline(never)]
pub(crate) fn persona_observe(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args)?;
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let context = opt_context(args, "context")?;
    let p_args = clio_persona::PersonaObservePreferenceArgs {
        store: &*state.persona,
        continuous: &*state.continuous,
        ctx: &ctx,
        key: req_str(args, "key")?,
        observed_value: opt_num(args, "observed_value").ok_or_else(|| {
            AmError::new(ErrorCode::InvalidArgument, "observed_value is required")
        })?,
        context: context.as_deref(),
        confidence: opt_num(args, "confidence"),
        source: opt_str(args, "source"),
        evidence_ref: opt_str(args, "evidence_ref"),
        policy: gated_write_policy(state, ctx.bank.as_str(), args)?,
        signal_source: &signals,
        admission_sink: &StderrSinks,
        continuous_sink: &StderrSinks,
        persona_sink: &StderrSinks,
        budget_tokens: state.budget_tokens,
        timestamp: &ts,
    };
    match clio_persona::persona_observe_preference(&p_args)? {
        clio_persona::PersonaObserveOutcome::Observed {
            state: new_state,
            trend,
            admission_score,
            dropped_keys,
        } => Ok(json!({
            "outcome": "observed",
            "state": new_state,
            "trend": trend,
            "admission_score": admission_score,
            "dropped_keys": dropped_keys
        })),
        clio_persona::PersonaObserveOutcome::Rejected { decision } => Ok(json!({
            "outcome": "rejected",
            "decision": serde_json::to_value(&decision).expect("serialize tool payload")
        })),
    }
}

/// Gated `task_upsert` (episodic admission).
#[inline(never)]
pub(crate) fn task_upsert(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args)?;
    let task_id = req_str(args, "task_id")?;
    let steps: Vec<String> = args
        .get("steps")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let context = opt_context(args, "context")?;
    let t_args = clio_history::TaskUpsertArgs {
        store: &*state.history,
        ctx: &ctx,
        subject_id: task_id,
        task_id,
        definition: req_str(args, "definition")?,
        steps: &steps,
        status: req_str(args, "status")?,
        context: context.as_deref(),
        evidence_ref: opt_str(args, "evidence_ref"),
        policy: gated_write_policy(state, ctx.bank.as_str(), args)?,
        signal_source: &signals,
        admission_sink: &StderrSinks,
        sink: &StderrSinks,
        timestamp: &ts,
        dry_run: opt_bool(args, "dry_run", false)?,
    };
    match clio_history::task_upsert(&t_args)? {
        clio_history::TaskUpsertOutcome::Stored { record, decision } => Ok(json!({
            "outcome": "stored",
            "record": serde_json::to_value(&record).expect("serialize tool payload"),
            "decision": serde_json::to_value(&decision).expect("serialize tool payload")
        })),
        clio_history::TaskUpsertOutcome::Rejected { decision } => Ok(json!({
            "outcome": "rejected",
            "decision": serde_json::to_value(&decision).expect("serialize tool payload")
        })),
        clio_history::TaskUpsertOutcome::DryRun {
            item,
            record,
            decision,
        } => Ok(json!({
            "outcome": "dry_run",
            "record": serde_json::to_value(&record).expect("serialize tool payload"),
            "carrier_id": item.id,
            "decision": serde_json::to_value(&decision).expect("serialize tool payload"),
            "dry_run": true
        })),
    }
}

/// Gated `failure_record` with the lesson token cap.
#[inline(never)]
pub(crate) fn failure_record(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args)?;
    let task_id = req_str(args, "task_id")?;
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let context = opt_context(args, "context")?;
    let f_args = clio_history::FailureRecordArgs {
        store: &*state.history,
        ctx: &ctx,
        subject_id: task_id,
        task_id,
        attempt_n: args
            .get("attempt_n")
            .and_then(Value::as_i64)
            .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "attempt_n is required"))?,
        what_failed: req_str(args, "what_failed")?,
        lesson: req_str(args, "lesson")?,
        context: context.as_deref(),
        evidence_ref: opt_str(args, "evidence_ref"),
        policy: gated_write_policy(state, ctx.bank.as_str(), args)?,
        history_policy: &state.history_policy,
        signal_source: &signals,
        admission_sink: &StderrSinks,
        sink: &StderrSinks,
        timestamp: &ts,
        dry_run: opt_bool(args, "dry_run", false)?,
    };
    match clio_history::failure_record(&f_args)? {
        clio_history::FailureRecordOutcome::Stored { record, decision } => Ok(json!({
            "outcome": "stored",
            "record": serde_json::to_value(&record).expect("serialize tool payload"),
            "decision": serde_json::to_value(&decision).expect("serialize tool payload")
        })),
        clio_history::FailureRecordOutcome::Rejected { decision } => Ok(json!({
            "outcome": "rejected",
            "decision": serde_json::to_value(&decision).expect("serialize tool payload")
        })),
        clio_history::FailureRecordOutcome::DryRun {
            item,
            record,
            decision,
        } => Ok(json!({
            "outcome": "dry_run",
            "record": serde_json::to_value(&record).expect("serialize tool payload"),
            "carrier_id": item.id,
            "decision": serde_json::to_value(&decision).expect("serialize tool payload"),
            "dry_run": true
        })),
    }
}

#[cfg(test)]
#[path = "mutator_record_tools_tests.rs"]
mod mutator_record_tools_tests;
