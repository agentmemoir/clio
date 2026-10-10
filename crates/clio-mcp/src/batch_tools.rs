// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `batch` tool: decoding ordered Core
//! write operations, pre-flighting every admission gate, resolving
//! `$ref:<index>.id` references topologically, and committing all staged
//! effects in ONE backend transaction.
//!
//! ## Owns
//! - Batch op decoding, `$ref` substitution, and the per-op report shape
//! - The pre-flight (score-without-write) wiring per recognized op tool
//!
//! ## Does not own
//! - Gate or admission math (in-process tools own behavior)
//! - The transactional executor (`clio_store::BatchStore`)
//! - Scratchpad/canonical/shared/validate handlers (see `additive_tools`)
//!
//! ## Boundary
//!
//! Changes to batch orchestration or report shape should normally require
//! changes here. Changes to admission or store semantics should not require
//! changes here.
//!
//! Keep this module focused on the atomic batch engine.

use crate::batch_preflight::preflight;
use crate::runtime::McpState;
use crate::write_tools::{attach_leaf, opt_bool, req_str, tool_result};
use clio_store::{BatchWrite, MAX_BATCH_OPS};
use clio_types::{AmError, ErrorCode, MemoryItem};
use serde_json::{Map, Value};

/// Recognized mutating Core tools inside `batch`.
const BATCH_TOOLS: [&str; 7] = [
    "store",
    "triple_add",
    "canonical_put",
    "persona_put_stable",
    "task_upsert",
    "failure_record",
    "graph_link",
];

/// One pre-flighted batch operation (scored without writing).
pub(crate) struct Staged {
    /// Staged row-level effects.
    pub(crate) writes: Vec<BatchWrite>,
    /// Durable item id this operation produces (for `$ref` targets).
    pub(crate) id: Option<String>,
    /// Admission score when the op was scored.
    pub(crate) admission_score: Option<f64>,
}

/// `batch(operations, dry_run?)`: atomic multi-mutation of Core write tools.
#[must_use]
pub fn batch_tool(state: &McpState, args: &Value) -> Value {
    tool_result(run_batch(state, args))
}

fn run_batch(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ops = args
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "operations array is required"))?;
    if ops.len() > MAX_BATCH_OPS {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("batch exceeds the {MAX_BATCH_OPS}-operation cap"),
        ));
    }
    let dry_run = opt_bool(args, "dry_run", false);
    let mut writes: Vec<BatchWrite> = Vec::new();
    let mut resolved: Vec<Option<String>> = Vec::new();
    let mut reports: Vec<Value> = Vec::new();
    let mut created_items: Vec<MemoryItem> = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let tool = req_str(op, "tool")?;
        if !BATCH_TOOLS.contains(&tool) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("operation {index}: unrecognized batch op tool `{tool}`"),
            ));
        }
        let empty = Value::Object(Map::new());
        let raw_params = op.get("params").unwrap_or(&empty);
        let in_batch_ids: Vec<String> = resolved.iter().flatten().cloned().collect();
        let params = substitute_refs(raw_params, &resolved)?;
        let staged = preflight(state, tool, &params, &in_batch_ids).map_err(|e| {
            AmError::new(
                e.code,
                format!(
                    "batch aborted at operation {index} (`{tool}`): {}",
                    e.message
                ),
            )
        })?;
        resolved.push(staged.id.clone());
        collect_carriers(tool, &staged.writes, &mut created_items);
        writes.extend(staged.writes);
        reports.push(serde_json::json!({
            "index": index,
            "tool": tool,
            "status": "STAGED",
            "id": staged.id,
            "admission_score": staged.admission_score
        }));
        // Carriers staged inside this op attach to the MemTree only after the
        // whole batch commits; pre-flight never mutates the forest.
    }
    if dry_run {
        for report in &mut reports {
            report["status"] = Value::from("WOULD_COMMIT");
        }
        return Ok(serde_json::json!({ "dry_run": true, "operations": reports }));
    }
    state.batch.apply_batch(&writes)?;
    // Attach + publish committed leaves so reads see them immediately.
    for item in &created_items {
        let signal = attach_leaf(state, item)?;
        state.dispatch_maintenance(vec![signal]);
    }
    for report in &mut reports {
        report["status"] = Value::from("COMMITTED");
    }
    Ok(Value::Object(Map::from_iter([(
        "operations".to_owned(),
        Value::Array(reports),
    )])))
}

/// Record the admitted carriers of each op type for post-commit attach.
#[allow(clippy::match_same_arms)]
fn collect_carriers(tool: &str, writes: &[BatchWrite], out: &mut Vec<MemoryItem>) {
    for write in writes {
        match (tool, write) {
            ("store", BatchWrite::CreateItem { item, .. })
            | ("canonical_put", BatchWrite::CanonicalPut { item, .. }) => {
                out.push(item.clone());
            }
            ("triple_add", BatchWrite::CommitTriple { item, .. }) => {
                out.push(item.clone());
            }
            ("task_upsert", BatchWrite::CommitTaskUpsert { item, .. })
            | ("failure_record", BatchWrite::CommitFailureRecord { item, .. }) => {
                out.push(item.clone());
            }
            _ => {}
        }
    }
}

/// Resolve `$ref:<index>.id` tokens against earlier staged operations.
fn substitute_refs(params: &Value, resolved: &[Option<String>]) -> Result<Value, AmError> {
    fn walk(value: &Value, resolved: &[Option<String>]) -> Result<Value, AmError> {
        Ok(match value {
            Value::String(s) => Value::String(replace_tokens(s, resolved)?),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|v| walk(v, resolved))
                    .collect::<Result<Vec<_>, AmError>>()?,
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| Ok((k.clone(), walk(v, resolved)?)))
                    .collect::<Result<Map<String, Value>, AmError>>()?,
            ),
            other => other.clone(),
        })
    }
    walk(params, resolved)
}

fn replace_tokens(text: &str, resolved: &[Option<String>]) -> Result<String, AmError> {
    let mut out = text.to_owned();
    for (index, id) in resolved.iter().enumerate() {
        let token = format!("$ref:{index}.id");
        if !text.contains(&token) {
            continue;
        }
        let Some(id) = id else {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "unresolved batch reference `{token}`: operation {index} produced no item id"
                ),
            ));
        };
        out = out.replace(&token, id);
    }
    // Any remaining token shape is a forward or malformed reference.
    if out.contains("$ref:") {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("malformed or forward batch reference in `{out}`"),
        ));
    }
    Ok(out)
}

#[cfg(test)]
#[path = "batch_tools_canonical_tests.rs"]
mod batch_tools_canonical_tests;
#[cfg(test)]
#[path = "batch_tools_record_op_tests.rs"]
mod batch_tools_record_op_tests;
#[cfg(test)]
#[path = "batch_tools_test_support_tests.rs"]
mod batch_tools_test_support_tests;
#[cfg(test)]
#[path = "batch_tools_tests.rs"]
mod batch_tools_tests;

#[cfg(test)]
#[path = "batch_tools_context_batch_canonical_tests.rs"]
mod batch_tools_context_batch_canonical_tests;
