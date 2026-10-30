// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP history, `MemTree`, triple, and belief
//! read handlers (task/failure history, temporal trajectories, `MemTree` nodes,
//! bi-temporal triples, belief confidence history).
//!
//! ## Owns
//! - `task_get`, `task_history`, `failures_for_task`, `memtree_query`,
//!   `memtree_get`, `temporal_history`, `triple_query`, `belief_history`
//!
//! ## Does not own
//! - History/triple/belief persistence (`clio-store`) or the façade semantics
//!   (`clio-history` / `clio-belief`)
//! - Retrieve/compose/item handlers (see `read_retrieve` / `read_tools`)
//!
//! ## Boundary
//!
//! Changes to history read binding or result shaping should normally require
//! changes here. Changes to storage/façade semantics should not.

use crate::read_tools::{opt_u32, opt_usize, time_axis};
use crate::runtime::{McpState, StderrSinks};
use crate::write_tools::{opt_str, req_str};
use clio_types::{AmError, ErrorCode, TimeAxis, TripleQuery};
use serde_json::{Value, json};

/// Route one history/graph read tool; `None` when not ours.
pub(crate) fn dispatch(
    state: &McpState,
    name: &str,
    args: &Value,
) -> Option<Result<Value, AmError>> {
    Some(match name {
        "task_get" => task_get(state, args),
        "task_history" => task_history(state, args),
        "failures_for_task" => failures_for_task(state, args),
        "memtree_query" => memtree_query(state, args),
        "memtree_get" => memtree_get(state, args),
        "temporal_history" => temporal_history(state, args),
        "triple_query" => triple_query(state, args),
        "belief_history" => belief_history(state, args),
        _ => return None,
    })
}

/// A task id the bank does not hold. A mistyped id is an error, so it cannot read
/// as a task with no history.
fn task_not_found(task_id: &str) -> AmError {
    AmError::new(
        ErrorCode::NotFound,
        format!("task `{task_id}` not found in bank"),
    )
}

/// Refuse a task id the bank does not hold.
fn require_task(state: &McpState, bank: &str, task_id: &str) -> Result<(), AmError> {
    match state.history.get_task(bank, task_id)? {
        Some(_) => Ok(()),
        None => Err(task_not_found(task_id)),
    }
}

/// `task_get(task_id)`: latest task version + optional ancestor gist.
#[inline(never)]
fn task_get(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let task_id = req_str(args, "task_id")?;
    let outcome = clio_history::task_get(&clio_history::TaskGetArgs {
        history: &*state.history,
        store: &*state.store,
        bank_id: ctx.bank.as_str(),
        task_id,
    })?;
    if outcome.record.is_none() {
        return Err(task_not_found(task_id));
    }
    Ok(json!({
        "record": outcome.record,
        "ancestor_gist": outcome.ancestor_gist
    }))
}

/// `task_history(task_id, limit?)`: prior versions, newest first, hard-capped.
#[inline(never)]
fn task_history(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let task_id = req_str(args, "task_id")?;
    let outcome = clio_history::task_history(&clio_history::TaskHistoryArgs {
        history: &*state.history,
        bank_id: ctx.bank.as_str(),
        task_id,
        limit: opt_usize(args, "limit"),
        policy: &state.history_policy,
    })?;
    if outcome.records.is_empty() {
        require_task(state, ctx.bank.as_str(), task_id)?;
    }
    Ok(json!({
        "records": outcome.records,
        "limit": outcome.limit
    }))
}

/// `failures_for_task(task_id_or_query, limit?)`: exact-first then similar.
#[inline(never)]
fn failures_for_task(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let query = req_str(args, "task_id_or_query")?;
    let outcome = clio_history::failures_for_task(&clio_history::FailuresForTaskArgs {
        history: &*state.history,
        bank_id: ctx.bank.as_str(),
        task_id_or_query: query,
        limit: opt_usize(args, "limit"),
        policy: &state.history_policy,
        sink: &StderrSinks,
    })?;
    // Nothing matched the argument as a task or as failure text: it names no task
    // the bank holds, so it is not found rather than an empty list.
    if outcome.mode == "none" {
        require_task(state, ctx.bank.as_str(), query)?;
    }
    Ok(json!({
        "failures": outcome.failures,
        "matched_task_ids": outcome.matched_task_ids,
        "mode": outcome.mode,
        "limit": outcome.limit
    }))
}

/// Hard cap on `memtree_query` depth (memory/DoS guard).
pub const MAX_MEMTREE_DEPTH: u32 = 32;

/// `memtree_query(time_range?, task_id?, depth?, limit?)`: bank-scoped nodes.
#[inline(never)]
fn memtree_query(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let depth = opt_u32(args, "depth");
    if let Some(d) = depth
        && d > MAX_MEMTREE_DEPTH
    {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!("memtree_query depth must be <= {MAX_MEMTREE_DEPTH}"),
        ));
    }
    let range = args.get("time_range");
    let filter = clio_write::MemTreeQueryFilter {
        time_start: range_bound(range, "start"),
        time_end: range_bound(range, "end"),
        depth,
        task_id: opt_str(args, "task_id").map(str::to_owned),
        limit: opt_usize(args, "limit"),
    };
    let result =
        clio_write::memtree_query(&*state.store, &state.forest, ctx.bank.as_str(), &filter);
    let nodes = crate::read_tools::tool_result_value(result)?;
    Ok(json!({ "nodes": nodes }))
}

/// Optional string field of the `time_range` object (`start` / `end`).
fn range_bound(range: Option<&Value>, key: &str) -> Option<String> {
    range
        .and_then(|r| r.get(key))
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// `memtree_get(node_id)`: fetch one `MemTree` node.
#[inline(never)]
fn memtree_get(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let result = clio_write::memtree_get(
        &*state.store,
        &state.forest,
        ctx.bank.as_str(),
        req_str(args, "node_id")?,
    );
    crate::read_tools::tool_result_value(result)
}

/// `temporal_history(target, as_of?, time_axis?)`: one trajectory façade.
#[inline(never)]
fn temporal_history(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let as_of = opt_str(args, "as_of");
    let outcome = clio_history::temporal_history(&clio_history::TemporalHistoryArgs {
        history: &*state.history,
        continuous: &*state.continuous,
        beliefs: &*state.belief,
        bank_id: ctx.bank.as_str(),
        target: req_str(args, "target")?,
        as_of,
        time_axis: time_axis(args)?,
        limit: None,
        policy: &state.history_policy,
        sink: &StderrSinks,
    })?;
    Ok(temporal_outcome_json(&outcome))
}

/// Serialize the non-`Serialize` temporal outcome into a tagged JSON payload.
///
/// Context is emitted only when present: the belief variant inserts
/// `context` into the payload only for `Some`, and preference points carry
/// their own `context` through serde (the key is skipped when `None`).
#[inline(never)]
fn temporal_outcome_json(outcome: &clio_history::TemporalHistoryOutcome) -> Value {
    match outcome {
        clio_history::TemporalHistoryOutcome::Triple {
            subject,
            predicate,
            entries,
            as_of,
            time_axis,
        } => json!({
            "target_kind": "triple",
            "subject": subject,
            "predicate": predicate,
            "entries": entries,
            "as_of": as_of,
            "time_axis": axis_str(*time_axis)
        }),
        clio_history::TemporalHistoryOutcome::Preference { key, points, as_of } => json!({
            "target_kind": "preference",
            "key": key,
            "points": points,
            "as_of": as_of
        }),
        clio_history::TemporalHistoryOutcome::Belief {
            belief_id,
            proposition,
            context,
            entries,
            as_of,
        } => {
            let mut payload = json!({
                "target_kind": "belief",
                "belief_id": belief_id,
                "proposition": proposition,
                "entries": entries,
                "as_of": as_of
            });
            if let (Some(text), Some(object)) = (context, payload.as_object_mut()) {
                object.insert("context".into(), Value::from(text.as_str()));
            }
            payload
        }
    }
}

#[inline(never)]
fn axis_str(axis: TimeAxis) -> &'static str {
    axis.as_str()
}

/// `triple_query(subject?, predicate?, object?, as_of?, time_axis?, limit?, offset?)`.
#[inline(never)]
fn triple_query(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let query = TripleQuery {
        bank_id: ctx.bank.as_str().to_owned(),
        subject: opt_str(args, "subject").map(str::to_owned),
        predicate: opt_str(args, "predicate").map(str::to_owned),
        object: opt_str(args, "object").map(str::to_owned),
        as_of: opt_str(args, "as_of").map(str::to_owned),
        time_axis: time_axis(args)?,
    };
    let rows = state.store.query_triples(&query)?;
    // Paging at the binding layer, as `inspect` does for its filter: slice the
    // matched rows, and echo the served page plus the full match count so the
    // CLI can mark a short page. Direct callers without `limit` get the same
    // 50-page default the CLI sends; the cap matches the inspect binding.
    let limit = opt_usize(args, "limit").unwrap_or(50).min(200);
    let offset = opt_usize(args, "offset").unwrap_or(0);
    let total = rows.len();
    let triples: Vec<_> = rows.into_iter().skip(offset).take(limit).collect();
    // A named subject that matched nothing is either a subject the bank does
    // not hold (not found) or a real one whose predicate/object pattern does
    // not match (an empty page). Re-read with the subject alone to tell the
    // two apart; the full-bank listing keeps its empty success.
    if triples.is_empty() && query.subject.is_some() {
        let existence = TripleQuery {
            subject: query.subject.clone(),
            predicate: None,
            object: None,
            ..query
        };
        if state.store.query_triples(&existence)?.is_empty() {
            let subject = existence.subject.as_deref().expect("named subject");
            return Err(AmError::new(
                ErrorCode::NotFound,
                format!("triple subject `{subject}` not found in bank"),
            ));
        }
    }
    Ok(json!({
        "triples": triples,
        "total": total,
        "limit": limit,
        "offset": offset,
    }))
}

/// `belief_history(belief_id_or_proposition)`: full confidence trajectory.
#[inline(never)]
fn belief_history(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let history = clio_belief::belief_history(
        &*state.belief,
        ctx.bank.as_str(),
        req_str(args, "belief_id_or_proposition")?,
        None,
    )?;
    Ok(serde_json::to_value(&history).expect("serialize belief history"))
}

#[cfg(test)]
#[path = "read_history_memtree_tests.rs"]
mod read_history_memtree_tests;

#[cfg(test)]
#[path = "read_history_temporal_context_tests.rs"]
mod read_history_temporal_context_tests;

#[cfg(test)]
#[path = "read_history_not_found_tests.rs"]
mod not_found_tests;
