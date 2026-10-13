// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the direct gated write handlers behind the
//! MCP write dispatcher: triples, beliefs, and explicit association edges,
//! each decoding its arguments and shaping the in-process result.
//!
//! ## Owns
//! - `triple_add` / `triple_end` handler argument decoding and result shaping
//! - `belief_observe` handler argument decoding and result shaping
//! - `graph_link` handler argument decoding
//!
//! ## Does not own
//! - Tool-name routing and envelope shaping (`write_tools`)
//! - Admission math and persistence (the in-process write tools)
//!
//! ## Boundary
//!
//! Changes to a triple, belief, or graph-edge handler's arguments or result
//! shape should normally require changes here. Changes to routing or the
//! shared argument readers should not require changes here.
//!
//! Keep this module focused on the direct write handlers.

use crate::runtime::{McpState, StderrSinks, StoreSignals};
use crate::write_tools::{
    end_outcome_json, gated_write_policy, opt_context, opt_num, opt_str, req_str,
};
use clio_types::{AmError, ErrorCode};
use serde_json::{Map, Value};
use std::sync::Arc;

/// Gated `triple_add` (episodic admission + atomic supersede+insert).
#[inline(never)]
pub(crate) fn triple_add(state: &McpState, args: &Value) -> Result<Value, AmError> {
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
        policy: gated_write_policy(state, ctx.bank.as_str(), args)?,
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
    out.insert("bank".into(), Value::from(ctx.bank.as_str()));
    Ok(Value::Object(out))
}

/// `triple_end`: expire open SPO edges without replacement.
#[inline(never)]
pub(crate) fn triple_end(state: &McpState, args: &Value) -> Result<Value, AmError> {
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
    Ok(with_bank(end_outcome_json(&outcome), ctx.bank.as_str()))
}

/// `belief_observe`: gated create / validated append.
#[inline(never)]
pub(crate) fn belief_observe(state: &McpState, args: &Value) -> Result<Value, AmError> {
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
        policy: gated_write_policy(state, ctx.bank.as_str(), args)?,
        sink: &StderrSinks,
        timestamp: &ts,
    };
    let outcome = clio_belief::belief_observe(&*state.belief, &signals, &b_args)?;
    Ok(with_bank(
        serde_json::to_value(&outcome).expect("serialize tool payload"),
        ctx.bank.as_str(),
    ))
}

/// Explicit association edge declaration (assoc engine).
///
/// Bank/actor resolve through [`McpState::resolve_ctx`], so a schema-conformant
/// call that omits `bank` uses the connection default (args win over defaults).
#[inline(never)]
pub(crate) fn graph_link(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let view = state.assoc.graph_link(
        ctx.bank.as_str(),
        req_str(args, "source_id")?,
        req_str(args, "target_id")?,
        req_str(args, "relationship")?,
        opt_num(args, "weight"),
        ctx.actor.as_str(),
    )?;
    Ok(with_bank(
        serde_json::to_value(&view).expect("serialize tool payload"),
        ctx.bank.as_str(),
    ))
}

/// Attach the resolved bank to a write result so the caller's receipt names the
/// bank the write landed in, whether the caller named one or took the default.
fn with_bank(mut value: Value, bank: &str) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("bank".into(), Value::from(bank));
    }
    value
}
