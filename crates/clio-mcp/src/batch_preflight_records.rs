// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the record-shaped batch pre-flights:
//! `persona_put_stable`, `task_upsert`, and `failure_record` operations
//! scored with `dry_run` and staged as persona/history batch writes.
//!
//! ## Owns
//! - Pre-flight staging for persona stable entries and task/failure records
//!
//! ## Does not own
//! - Item/triple/canonical/graph pre-flights (see `batch_preflight`)
//! - The batch orchestrator (`batch_tools`)
//!
//! ## Boundary
//!
//! Changes to how these records stage into batch writes should normally
//! require changes here. Changes to admission or history semantics should not.
//!
//! Keep this module focused on record-shaped batch pre-flights.

use crate::batch_tools::Staged;
use crate::runtime::{McpState, StderrSinks, StoreSignals};
use crate::write_tools::{gated_write_policy, opt_context, opt_num, opt_str, req_str};
use clio_store::BatchWrite;
use clio_types::{AmError, ErrorCode, OpContext};
use serde_json::Value;
use std::sync::Arc;

pub(crate) fn rejection(decision: &clio_admission::decision::Decision) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        decision
            .rejection_reason
            .clone()
            .unwrap_or_else(|| "admission rejected".into()),
    )
}

fn actor_of(ctx: &OpContext) -> String {
    ctx.actor.as_str().to_owned()
}

pub(crate) fn records_preflight(
    state: &McpState,
    tool: &str,
    ctx: &OpContext,
    ts: &str,
    params: &Value,
) -> Result<Staged, AmError> {
    match tool {
        "persona_put_stable" => persona_preflight(state, ctx, ts, params),
        "task_upsert" => task_preflight(state, ctx, ts, params),
        "failure_record" => failure_preflight(state, ctx, ts, params),
        _ => Err(AmError::new(
            ErrorCode::Internal,
            format!("unreachable batch tool `{tool}`"),
        )),
    }
}

fn persona_preflight(
    state: &McpState,
    ctx: &OpContext,
    ts: &str,
    params: &Value,
) -> Result<Staged, AmError> {
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let context = opt_context(params, "context")?;
    let p_args = clio_persona::PersonaPutStableArgs {
        store: &*state.persona,
        continuous: &*state.continuous,
        ctx,
        key: req_str(params, "key")?,
        value: req_str(params, "value")?,
        context: context.as_deref(),
        confidence: opt_num(params, "confidence"),
        source: opt_str(params, "source"),
        evidence_ref: opt_str(params, "evidence_ref"),
        policy: gated_write_policy(state, ctx.bank.as_str(), params)?,
        signal_source: &signals,
        admission_sink: &StderrSinks,
        persona_sink: &StderrSinks,
        budget_tokens: state.budget_tokens,
        timestamp: ts,
        dry_run: true,
    };
    match clio_persona::persona_put_stable(&p_args)? {
        clio_persona::PersonaPutStableOutcome::DryRun { entry, decision } => Ok(Staged {
            writes: vec![BatchWrite::PutStableEntry {
                entry: *entry,
                actor: actor_of(ctx),
            }],
            id: None,
            admission_score: Some(decision.admission_score),
        }),
        clio_persona::PersonaPutStableOutcome::Rejected { decision } => Err(rejection(&decision)),
        clio_persona::PersonaPutStableOutcome::Admitted { .. } => Err(AmError::new(
            ErrorCode::Internal,
            "persona pre-flight must not persist",
        )),
    }
}

fn task_preflight(
    state: &McpState,
    ctx: &OpContext,
    ts: &str,
    params: &Value,
) -> Result<Staged, AmError> {
    let task_id = req_str(params, "task_id")?;
    let steps: Vec<String> = params
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
    let context = opt_context(params, "context")?;
    let t_args = clio_history::TaskUpsertArgs {
        store: &*state.history,
        ctx,
        subject_id: opt_str(params, "subject_id").unwrap_or(task_id),
        task_id,
        definition: req_str(params, "definition")?,
        steps: &steps,
        status: req_str(params, "status")?,
        context: context.as_deref(),
        evidence_ref: opt_str(params, "evidence_ref"),
        policy: gated_write_policy(state, ctx.bank.as_str(), params)?,
        signal_source: &signals,
        admission_sink: &StderrSinks,
        sink: &StderrSinks,
        timestamp: ts,
        dry_run: true,
    };
    match clio_history::task_upsert(&t_args)? {
        clio_history::TaskUpsertOutcome::DryRun {
            item,
            record,
            decision,
        } => {
            let id = record.item_id.clone();
            Ok(Staged {
                writes: vec![BatchWrite::CommitTaskUpsert {
                    item,
                    record,
                    actor: actor_of(ctx),
                }],
                id: Some(id),
                admission_score: Some(decision.admission_score),
            })
        }
        clio_history::TaskUpsertOutcome::Rejected { decision } => Err(rejection(&decision)),
        clio_history::TaskUpsertOutcome::Stored { .. } => Err(AmError::new(
            ErrorCode::Internal,
            "task pre-flight must not persist",
        )),
    }
}

fn failure_preflight(
    state: &McpState,
    ctx: &OpContext,
    ts: &str,
    params: &Value,
) -> Result<Staged, AmError> {
    let task_id = req_str(params, "task_id")?;
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let context = opt_context(params, "context")?;
    let f_args = clio_history::FailureRecordArgs {
        store: &*state.history,
        ctx,
        subject_id: opt_str(params, "subject_id").unwrap_or(task_id),
        task_id,
        attempt_n: params
            .get("attempt_n")
            .and_then(Value::as_i64)
            .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "attempt_n is required"))?,
        what_failed: req_str(params, "what_failed")?,
        lesson: req_str(params, "lesson")?,
        context: context.as_deref(),
        evidence_ref: opt_str(params, "evidence_ref"),
        policy: gated_write_policy(state, ctx.bank.as_str(), params)?,
        history_policy: &state.history_policy,
        signal_source: &signals,
        admission_sink: &StderrSinks,
        sink: &StderrSinks,
        timestamp: ts,
        dry_run: true,
    };
    match clio_history::failure_record(&f_args)? {
        clio_history::FailureRecordOutcome::DryRun {
            item,
            record,
            decision,
        } => {
            let id = record.item_id.clone();
            Ok(Staged {
                writes: vec![BatchWrite::CommitFailureRecord {
                    item,
                    record,
                    actor: actor_of(ctx),
                }],
                id: Some(id),
                admission_score: Some(decision.admission_score),
            })
        }
        clio_history::FailureRecordOutcome::Rejected { decision } => Err(rejection(&decision)),
        clio_history::FailureRecordOutcome::Stored { .. } => Err(AmError::new(
            ErrorCode::Internal,
            "failure pre-flight must not persist",
        )),
    }
}

#[cfg(test)]
#[path = "batch_preflight_records_tests.rs"]
mod batch_preflight_records_tests;
