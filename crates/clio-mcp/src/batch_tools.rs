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
//! - The command-level `bank`/`actor` defaults each operation inherits
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
    let dry_run = opt_bool(args, "dry_run", false)?;
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
        // The command-level `bank`/`actor` are defaults for every operation; an
        // operation that names its own keeps it. Without this the tool scored
        // each operation against the session defaults and stored the record in
        // the wrong bank under the wrong actor.
        let params = with_command_context(args, &params);
        // The same argument contract as a direct call. An undeclared or blank
        // argument aborts the batch, instead of being dropped and written without it.
        crate::arg_contract::check(tool, &params).map_err(|e| {
            AmError::new(
                e.code,
                format!(
                    "batch aborted at operation {index} (`{tool}`): {}",
                    e.message
                ),
            )
        })?;
        // `created_items` carries the operations before this one, so a repeat of
        // an earlier operation's text fails the duplicate gate instead of both
        // rows committing.
        let staged =
            preflight(state, tool, &params, &in_batch_ids, &created_items).map_err(|e| {
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

/// Apply the batch call's `bank`/`actor` to one operation as defaults.
///
/// An operation that names its own `bank` or `actor` keeps it; one that omits
/// either inherits the command-level value, which is what the CLI advertises.
/// Other arguments are untouched.
fn with_command_context(batch_args: &Value, params: &Value) -> Value {
    let Value::Object(mut fields) = params.clone() else {
        return params.clone();
    };
    for key in ["bank", "actor"] {
        if !fields.contains_key(key)
            && let Some(value) = batch_args.get(key)
        {
            fields.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(fields)
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

#[cfg(test)]
#[path = "batch_tools_contract_tests.rs"]
mod batch_tools_contract_tests;

#[cfg(test)]
#[path = "batch_tools_command_context_tests.rs"]
mod batch_tools_command_context_tests;

#[cfg(test)]
#[path = "batch_tools_duplicate_tests.rs"]
mod batch_tools_duplicate_tests;
