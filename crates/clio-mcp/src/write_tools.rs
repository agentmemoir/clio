// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP write-surface dispatcher: mapping
//! `tools/call` names to the in-process gated write handlers (admission,
//! store, triples, beliefs, explicit graph) and shaping their results.
//!
//! ## Owns
//! - Tool-name routing for the write set
//! - JSON argument decoding into in-process arg structs (bank/actor precedence)
//! - Store-path leaf attach + publish hand-off
//!
//! ## Does not own
//! - Gate or admission math (lives in the in-process tools)
//! - Mutator/persona/history handlers (see `mutator_tools`/`mutator_record_tools`)
//! - Transport framing (see `protocol`/`stdio`/`http`)
//!
//! ## Boundary
//!
//! Changes to MCP argument decoding or result shaping should normally require
//! changes here. Changes to admission or store semantics should not require
//! changes here.
//!
//! Keep this module focused on the MCP write dispatcher.

use crate::runtime::{McpState, StderrSinks, StoreSignals};
use clio_admission::decision::Candidate;
use clio_admission::policy::AdmissionPolicy;
use clio_types::{
    AmError, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory,
    ToolResult,
};
use clio_write::MaintenanceSignal;
use clio_write::memtree::AttachLeaf;
use serde_json::{Map, Value};
use std::sync::Arc;

/// Tools whose in-process mutator implements a real no-write `dry_run` path.
const DRY_RUN_TOOLS: [&str; 9] = [
    "store",
    "batch",
    "canonical_put",
    "persona_put_stable",
    "task_upsert",
    "failure_record",
    "import",
    "import_provider",
    "ranking_env_set",
];

/// True when `dry_run=true` targets a mutating tool with no no-write path.
///
/// The MCP adapter must never silently perform a durable write when a caller
/// asked for a dry run, so an unsupported `dry_run` fails closed instead.
fn dry_run_unsupported(name: &str, args: &Value) -> bool {
    args.get("dry_run").and_then(Value::as_bool) == Some(true)
        && name != "admit_preview"
        && !DRY_RUN_TOOLS.contains(&name)
}

/// Route one bound write tool to its in-process handler.
#[must_use]
#[inline(never)]
pub fn dispatch(state: &McpState, name: &str, args: &Value) -> Value {
    if crate::ops_tools::is_ops_tool(name) {
        return crate::ops_tools::dispatch_ops(state, name, args);
    }
    if crate::bound_read_tools().contains(&name) {
        return tool_result(crate::read_tools::dispatch(state, name, args));
    }
    if dry_run_unsupported(name, args) {
        return tool_result(Err(AmError::new(
            ErrorCode::NotImplemented,
            format!("dry_run is not supported for `{name}`: no in-process no-write path exists"),
        )));
    }
    if name == "batch" {
        return nudge_if_ok(state, crate::batch_tools::batch_tool(state, args));
    }
    let result = match name {
        "admit_preview" => crate::store_write::admit_preview(state, args),
        "store" => crate::store_write::store(state, args),
        "triple_add" => triple_add(state, args),
        "triple_end" => triple_end(state, args),
        "belief_observe" => belief_observe(state, args),
        "graph_link" => graph_link(state, args),
        "config_set" | "config_profile_apply" | "ranking_env_set" => {
            crate::config_tools::dispatch_write(state, name, args)
        }
        other => crate::additive_tools::dispatch_inner(state, other, args),
    };
    nudge_if_ok(state, tool_result(result))
}

/// Nudge the index sweeper when a mutating tool call was accepted.
///
/// A successful mutation may have enqueued durable index jobs; the nudge is an
/// O(1) flag set so the write response never waits on embedding.
#[inline(never)]
fn nudge_if_ok(state: &McpState, value: Value) -> Value {
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        state.nudge_index();
    }
    value
}

#[inline(never)]
pub(crate) fn tool_result(result: Result<Value, AmError>) -> Value {
    let shaped: ToolResult<Value> = match result {
        Ok(data) => ToolResult::success(data),
        Err(err) => ToolResult::failure(err),
    };
    serde_json::to_value(shaped).unwrap_or_else(|_| {
        serde_json::to_value(ToolResult::<()>::failure(AmError::new(
            ErrorCode::Internal,
            "serialize failed",
        )))
        .expect("failure envelope serializes")
    })
}

/// Required non-empty string argument.
pub(crate) fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, AmError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, format!("{key} is required")))
}

/// Optional non-empty string argument.
pub(crate) fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
}

/// Optional bounded `context` argument (absent/null -> None).
///
/// A present value must be a non-empty string within the published UTF-8 byte
/// maximum; an explicitly empty string is rejected, not treated as absent.
pub(crate) fn opt_context(args: &Value, key: &str) -> Result<Option<String>, AmError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            clio_types::validate_context(value)?;
            Ok(Some(value.clone()))
        }
        Some(_) => Err(AmError::new(
            ErrorCode::InvalidType,
            format!("{key} must be a string"),
        )),
    }
}

/// Optional number argument.
pub(crate) fn opt_num(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

/// Boolean argument with a default.
pub(crate) fn opt_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// Admission policy snapshot for one call, honoring the bank's retention profile.
pub(crate) fn admission_policy<'a>(state: &'a McpState, bank: &str) -> AdmissionPolicy<'a> {
    AdmissionPolicy::with_retention(&state.env, state.resolved_retention(bank))
}

/// The policy for a gated write, with the caller's duplicate waiver applied.
///
/// Every gated long-term write can hit the unconditional exact-duplicate
/// rejection, so every one of them must read the same `allow_duplicate`
/// argument. Routing them all through this helper is what keeps a new surface
/// from silently shipping without an escape hatch.
pub(crate) fn gated_write_policy<'a>(
    state: &'a McpState,
    bank: &str,
    args: &Value,
) -> AdmissionPolicy<'a> {
    admission_policy(state, bank).with_force(opt_bool(args, "allow_duplicate", false))
}

/// Build the candidate item shell for `store` / `admit_preview`.
#[inline(never)]
pub(crate) fn build_item(
    state: &McpState,
    ctx: &clio_types::OpContext,
    args: &Value,
    item_id: String,
) -> Result<MemoryItem, AmError> {
    let shell = args
        .get("item")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let ts = state.resolve_timestamp(args);
    let category = match opt_str(args, "category") {
        Some(raw) => Some(SemanticCategory::parse(raw)?),
        None => None,
    };
    let episodic = match opt_str(args, "episodic_type") {
        Some(raw) => Some(EpisodicType::parse(raw)?),
        None => None,
    };
    let kind = if episodic.is_some() {
        ItemKind::Episodic
    } else {
        ItemKind::Semantic
    };
    let subject_id = shell
        .get("subject_id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map_or_else(|| item_id.clone(), str::to_owned);
    Ok(MemoryItem {
        id: item_id,
        bank_id: ctx.bank.as_str().into(),
        subject_id,
        kind,
        category,
        episodic_type: episodic,
        epistemic_kind: EpistemicKind::parse(req_str(args, "epistemic_kind")?)?,
        source_type: opt_str(args, "source_type")
            .map(clio_types::SourceType::parse)
            .transpose()?,
        confidence: opt_num(args, "confidence"),
        admission_score: None,
        source_ref: None,
        snapshot: shell.get("snapshot").cloned().filter(|v| !v.is_null()),
        gist: shell
            .get("gist")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned),
        valid_from: ts.clone(),
        valid_until: None,
        tx_from: ts.clone(),
        tx_until: None,
        created_at: ts.clone(),
        updated_at: ts,
        context: opt_context(args, "context")?,
    })
}

/// Resolve the admission candidate: category for semantic, episodic tag otherwise.
pub(crate) fn build_candidate(args: &Value) -> Result<Candidate, AmError> {
    if let Some(raw) = opt_str(args, "category") {
        return Candidate::parse_semantic(raw);
    }
    if let Some(raw) = opt_str(args, "episodic_type") {
        return Candidate::parse_episodic(raw);
    }
    Err(AmError::new(
        ErrorCode::InvalidArgument,
        "category or episodic_type is required",
    ))
}

/// Attach the admitted leaf (queryable before ancestor refresh) and
/// publish it for immediate reads.
#[inline(never)]
pub(crate) fn attach_leaf(
    state: &McpState,
    item: &MemoryItem,
) -> Result<MaintenanceSignal, AmError> {
    let bank = item.bank_id.clone();
    let attach = AttachLeaf {
        item_id: item.id.clone(),
        bank_id: bank.clone(),
        interval_start: item.created_at.clone(),
        interval_end: item
            .valid_until
            .clone()
            .unwrap_or_else(|| item.created_at.clone()),
        gist: item.gist.clone(),
        task_id: None,
    };
    {
        let mut forest = state
            .forest
            .lock()
            .map_err(|_| AmError::new(ErrorCode::Internal, "memtree forest poisoned"))?;
        forest.tree_mut(&bank).attach_leaf(attach)?;
    }
    state.leaves.publish(item.clone());
    Ok(MaintenanceSignal::leaf_attached(item.id.clone(), bank))
}

/// Gated `triple_add` (episodic admission + atomic supersede+insert).
#[inline(never)]
fn triple_add(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let subject = req_str(args, "subject")?;
    let update_rule = match opt_str(args, "update_rule") {
        Some(raw) => Some(clio_types::UpdateRule::parse(raw)?),
        None => None,
    };
    let sink = StderrSinks;
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let context = opt_context(args, "context")?;
    let t_args = clio_write::triple::TripleAddArgs {
        ctx: &ctx,
        subject,
        predicate: req_str(args, "predicate")?,
        object: req_str(args, "object")?,
        epistemic_kind: clio_types::EpistemicKind::parse(req_str(args, "epistemic_kind")?)?,
        context: context.as_deref(),
        source_type: opt_str(args, "source_type")
            .map(clio_types::SourceType::parse)
            .transpose()?,
        confidence: opt_num(args, "confidence"),
        valid_from: opt_str(args, "valid_from"),
        valid_until: opt_str(args, "valid_until"),
        supersede: args
            .get("supersede")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        snapshot_ref: opt_str(args, "snapshot_ref"),
        gist_ref: opt_str(args, "gist_ref"),
        update_rule,
        subject_id: opt_str(args, "subject_id").unwrap_or(subject),
        policy: gated_write_policy(state, ctx.bank.as_str(), args),
        admit_sink: &sink,
        triple_sink: &sink,
        timestamp: &ts,
        dry_run: false,
    };
    let triple = clio_write::triple::triple_add(&*state.store, &signals, &t_args)?;
    let (decision, outcome) = (triple.decision, triple.outcome);
    let mut out = Map::new();
    out.insert(
        "decision".into(),
        serde_json::to_value(&decision).expect("serialize tool payload"),
    );
    if let Some(o) = outcome {
        out.insert("new_edge_id".into(), Value::from(o.new_edge_id));
        out.insert(
            "prior_edge_ids".into(),
            serde_json::to_value(&o.prior_edge_ids).expect("serialize tool payload"),
        );
    }
    Ok(Value::Object(out))
}

/// `triple_end`: expire open SPO edges without replacement.
#[inline(never)]
fn triple_end(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let update_rule = match opt_str(args, "update_rule") {
        Some(raw) => Some(clio_types::UpdateRule::parse(raw)?),
        None => None,
    };
    let outcome = clio_write::triple::triple_end(
        &*state.store,
        &ctx,
        req_str(args, "subject")?,
        req_str(args, "predicate")?,
        opt_str(args, "object"),
        req_str(args, "valid_until")?,
        &ts,
        update_rule,
        &StderrSinks,
    )?;
    Ok(end_outcome_json(&outcome))
}

/// Render a `TripleEndOutcome` as plain JSON.
#[must_use]
pub(crate) fn end_outcome_json(outcome: &clio_types::TripleEndOutcome) -> Value {
    serde_json::json!({
        "closed_ids": outcome.closed_ids,
        "already_ended": outcome.already_ended,
        "replacement_id": outcome.replacement_id
    })
}

/// `belief_observe`: gated create / validated append.
#[inline(never)]
fn belief_observe(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let proposition = req_str(args, "proposition")?;
    let subject_default = format!("belief:{proposition}");
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let context = opt_context(args, "context")?;
    let b_args = clio_belief::BeliefObserveArgs {
        ctx: &ctx,
        subject_id: opt_str(args, "subject_id").unwrap_or(&subject_default),
        proposition,
        confidence: opt_num(args, "confidence")
            .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "confidence is required"))?,
        context: context.as_deref(),
        evidence_ref: req_str(args, "evidence_ref")?,
        source_type: req_str(args, "source_type")?,
        as_of: opt_str(args, "as_of"),
        policy: gated_write_policy(state, ctx.bank.as_str(), args),
        sink: &StderrSinks,
        timestamp: &ts,
    };
    let outcome = clio_belief::belief_observe(&*state.belief, &signals, &b_args)?;
    Ok(serde_json::to_value(&outcome).expect("serialize tool payload"))
}

/// Explicit association edge declaration (assoc engine).
///
/// Bank/actor resolve through [`McpState::resolve_ctx`], so a schema-conformant
/// call that omits `bank` uses the connection default (args win over defaults).
#[inline(never)]
fn graph_link(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let view = state.assoc.graph_link(
        ctx.bank.as_str(),
        req_str(args, "source_id")?,
        req_str(args, "target_id")?,
        req_str(args, "relationship")?,
        opt_num(args, "weight"),
        ctx.actor.as_str(),
    )?;
    Ok(serde_json::to_value(&view).expect("serialize tool payload"))
}

#[cfg(test)]
#[path = "write_tools_tests.rs"]
mod write_tools_tests;

#[cfg(test)]
#[path = "write_tools_scope_tests.rs"]
mod write_tools_scope_tests;

#[cfg(test)]
#[path = "write_tools_context_correct_tests.rs"]
mod write_tools_context_correct_tests;

#[cfg(test)]
#[path = "write_tools_context_domain_record_tests.rs"]
mod write_tools_context_domain_record_tests;

#[cfg(test)]
#[path = "write_tools_context_record_decode_tests.rs"]
mod write_tools_context_record_decode_tests;

#[cfg(test)]
#[path = "write_tools_context_write_tests.rs"]
mod write_tools_context_write_tests;
