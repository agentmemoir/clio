// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP-bound hygiene tools:
//!  `hygiene_audit` (read-only ranked noise scan), `hygiene_clean`
//! (confirmed `flag/archive/discard/apply_suggested`), and `hygiene_log_list`
//! (masked durable log rows), decoded from MCP args into in-process calls.
//!
//! ## Owns
//! - JSON argument decoding and result shaping for the three hygiene tools
//! - `delete`-alias mapping and candidate/`item_ids` target normalization
//! - Secret masking on every response tree (defense in depth over the
//!   already-redacted service outputs)
//!
//! ## Does not own
//! - Scoring, scanning, or clean execution semantics (see `clio-hygiene`)
//! - Confirmation gating internals (the in-process clean owns the gate)
//!
//! ## Boundary
//!
//! Changes to MCP argument decoding or result shaping should normally require
//! changes here. Changes to hygiene service behavior should not require
//! changes here.
//!
//! Keep this module focused on MCP hygiene dispatch.

use crate::runtime::{McpState, now_utc};
use crate::write_tools::{opt_bool, opt_num, opt_str};
use clio_hygiene::{CleanAction, CleanArgs, CleanTarget};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};

/// Route a hygiene tool name to its in-process handler (raw result).
#[inline(never)]
pub(crate) fn dispatch_inner(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    match name {
        "hygiene_audit" => hygiene_audit_tool(state, args),
        "hygiene_clean" => hygiene_clean_tool(state, args),
        "hygiene_log_list" => hygiene_log_list_tool(state, args),
        other => Err(AmError::not_implemented(other)),
    }
}

/// `hygiene_audit(bank?, min_score?, limit?, offset?)`: ranked read-only scan.
///
/// Must not mutate memory; the response carries masked previews and the
/// candidate ids later passed to `hygiene_clean`.
#[inline(never)]
fn hygiene_audit_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let bank = ctx.bank.as_str();
    let min_score = opt_num(args, "min_score").unwrap_or(clio_hygiene::DEFAULT_MIN_SCORE);
    let report = clio_hygiene::hygiene_audit(&clio_hygiene::AuditArgs {
        store: &*state.store,
        bank,
        now: &state.resolve_timestamp(args)?,
        min_score,
        limit: usize_arg(args, "limit").unwrap_or(50),
        offset: usize_arg(args, "offset").unwrap_or(0),
        weights: &state.env.hygiene,
        stale_half_life_days: state.env.hygiene_stale_half_life_days,
        max_scan: clio_hygiene::DEFAULT_MAX_SCAN,
    })?;
    Ok(serde_json::to_value(&report).expect("serialize tool payload"))
}

/// `hygiene_clean(candidates|item_ids, action?, confirm?, reason?, note?)`.
///
/// Without `confirm` the call is dry-run: counts only, zero writes. Targets
/// come from a prior audit (`candidates`) or bare ids (`item_ids`); `delete`
/// is accepted as the documented non-compliance alias of `discard`.
#[inline(never)]
fn hygiene_clean_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args)?;
    let action_raw = opt_str(args, "action").unwrap_or("apply_suggested");
    let action = CleanAction::parse(action_raw)?;
    let targets = clean_targets(args)?;
    let confirm = opt_bool(args, "confirm", false)?;
    let batch_id = state.next_clean_batch_id();
    let outcome = clio_hygiene::clean(&CleanArgs {
        store: &*state.store,
        hygiene: &*state.hygiene,
        ctx: &ctx,
        targets: &targets,
        action,
        confirm,
        reason: opt_str(args, "reason").unwrap_or("hygiene noise cleanup"),
        timestamp: &ts,
        batch_id: &batch_id,
        note: opt_str(args, "note"),
    })?;
    Ok(serde_json::to_value(&outcome).expect("serialize tool payload"))
}

/// `hygiene_log_list(limit?, offset?, bank?)`: masked durable log rows.
///
/// Each row carries `timestamp` / `bank` aliases of the store columns
/// `created_at` / `bank_id`, matching the log-entry terminology.
#[inline(never)]
fn hygiene_log_list_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let rows = state.hygiene.list_hygiene_logs(
        Some(ctx.bank.as_str()),
        usize_arg(args, "limit").unwrap_or(50),
        usize_arg(args, "offset").unwrap_or(0),
    )?;
    // Mask the tree defensively: rows are built id/score/label-only by
    // construction, so a masked_clone must be a no-op — a change here means a
    // log writer started leaking, and fail-closed masking keeps output safe.
    let rows: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut row = clio_config::secret::masked_clone(
                &serde_json::to_value(r).expect("serialize log row"),
            );
            if let Some(obj) = row.as_object_mut() {
                if let Some(created) = obj.get("created_at").cloned() {
                    obj.insert("timestamp".into(), created);
                }
                if let Some(bank) = obj.get("bank_id").cloned() {
                    obj.insert("bank".into(), bank);
                }
            }
            row
        })
        .collect();
    Ok(json!({ "rows": rows, "count": rows.len(), "timestamp": now_utc() }))
}

/// Normalize MCP targets: prior audit candidates or bare `item_ids`.
fn clean_targets(args: &Value) -> Result<Vec<CleanTarget>, AmError> {
    let mut targets = Vec::new();
    if let Some(candidates) = args.get("candidates").and_then(Value::as_array) {
        for c in candidates {
            let item_id = c
                .get("item_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AmError::new(ErrorCode::InvalidArgument, "candidate.item_id is required")
                })?
                .to_owned();
            targets.push(CleanTarget {
                item_id,
                noise_score: c.get("noise_score").and_then(Value::as_f64),
                suggested_action: c
                    .get("suggested_action")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }
    if let Some(ids) = args.get("item_ids").and_then(Value::as_array) {
        for id in ids {
            let item_id = id
                .as_str()
                .ok_or_else(|| {
                    AmError::new(
                        ErrorCode::InvalidArgument,
                        "item_ids entries must be strings",
                    )
                })?
                .to_owned();
            targets.push(CleanTarget {
                item_id,
                noise_score: None,
                suggested_action: None,
            });
        }
    }
    Ok(targets)
}

fn usize_arg(args: &Value, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
}

#[cfg(test)]
#[path = "hygiene_tools_bank_scope_tests.rs"]
mod hygiene_tools_bank_scope_tests;
#[cfg(test)]
#[path = "hygiene_tools_principal_tests.rs"]
mod hygiene_tools_principal_tests;
#[cfg(test)]
#[path = "hygiene_tools_tests.rs"]
mod hygiene_tools_tests;
