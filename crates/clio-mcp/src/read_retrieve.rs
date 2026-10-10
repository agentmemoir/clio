// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP retrieval, intent-gate, context
//! composition, and association-graph read handlers.
//!
//! ## Owns
//! - `intent_gate`, `retrieve`, `compose_context`, `associations`, `graph_query`
//! - Binding the `retrieve` read controls: the optional `min_scores` object
//!   through the one shared `min_scores_from_value` entry point, the
//!   `include_entities` and `entity_summary` toggles at their documented
//!   defaults, the `include_exact` rendering toggle at its `false` default,
//!   and the temporal-window `query_anchor` passed through unchanged
//!   (`clio-retrieve` owns the arm and its RFC 3339 validation)
//! - The bank retention profile's read-side fields reaching the request: the
//!   scope and read tolerance through `read_policy`, the curve state through
//!   `read_curve`, both resolved by `retention_effective`
//! - Binding the two bank-defaulted controls onto the wire: the provenance
//!   floor and the pack channel, each read from its own argument at
//!   `crate::read_read_policy`, which owns their resolution. `compose_context`
//!   resolves the floor for its own retrieval and publishes no floor argument,
//!   and takes the channel from its own `fidelity` argument.
//! - `McpState`'s `ReadPolicyResolver` impl: the same effective profile, for
//!   in-process callers of the `clio-retrieve` JSON surface
//! - The engine clock: the read handlers never read a timestamp argument for
//!   it (the published schemas declare none), so the request carries no `now`
//!   and the engine keeps its system-clock fallback; tests fix the clock
//!   through the `*_at` handler seams
//! - `explain` trace assembly at the binding: the top-level counters and
//!   timings (PII-safe ids/scores/counts only; entity names are content and
//!   stay out of it), including the optional `temporal_candidates` counter when
//!   the temporal arm ran. The per-hit shape comes from `crate::read_explain`.
//!
//! ## Does not own
//! - Retrieval/fusion/gate algorithms (domain crates own those)
//! - The floor rules and the entity-summary rules (`clio-retrieve` `floors`
//!   and `entity_summary`)
//! - How a bank-defaulted read control resolves from its arguments
//!   (`crate::read_read_policy` owns that)
//! - Item/history/inspect handlers (see `read_tools` / `read_history`)
//!
//! ## Boundary
//!
//! Changes to retrieval read binding or the trace's top-level shape should
//! normally require changes here. Changes to per-hit trace shaping belong in
//! `crate::read_explain`; changes to how a bank read-side control resolves
//! belong in `crate::read_read_policy`; changes to ranking algorithms should
//! not require changes here.
//!
//! Keep this module focused on MCP read-handler binding.

use crate::read_explain::explain_hit;
use crate::read_read_policy::{read_fidelity, read_provenance_floor};
use crate::read_tools::{opt_u32, opt_usize, str_list, time_axis};
use crate::runtime::McpState;
use crate::write_tools::{opt_bool, opt_num, opt_str, req_str};
use clio_config::retention::{DuplicateTolerance, RecallScope, RetentionProfile};
use clio_retrieve::{
    ComposePack, ComposeRequest, Composer, DEFAULT_BUDGET_TOKENS, DEFAULT_LIMIT, PersonaEntry,
    ReadPolicyResolver, RetrieveOutcome, RetrieveRequest, min_scores_from_value,
};
use clio_store::AssocKind;
use clio_types::{AmError, ErrorCode, OpContext};
use serde_json::{Value, json};

/// Hard cap on `graph_query` traversal depth (`DoS` guard).
pub const MAX_GRAPH_HOPS: usize = 3;

/// Route one retrieve/compose/graph read tool; `None` when not ours.
pub(crate) fn dispatch(
    state: &McpState,
    name: &str,
    args: &Value,
) -> Option<Result<Value, AmError>> {
    Some(match name {
        "intent_gate" => intent_gate(state, args),
        "retrieve" => retrieve(state, args),
        "compose_context" => compose_context(state, args),
        "associations" => associations(state, args),
        "graph_query" => graph_query(state, args),
        _ => return None,
    })
}

/// `intent_gate(turn_text)`: diagnostic gate decision never injects.
#[inline(never)]
fn intent_gate(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let decision = clio_retrieve::intent_gate(req_str(args, "turn_text")?, &state.env.intent_gate);
    Ok(json!({
        "retrieval_needed": decision.retrieval_needed,
        "domains": decision.domains,
        "confidence": decision.confidence,
        "reason": decision.reason
    }))
}

/// `retrieve(...)`: explicit retrieval; never gated.
///
/// The optional read controls are bound here and nowhere else: the score floors
/// through the one shared entry point [`min_scores_from_value`], so a malformed
/// or out-of-range floor fails closed with the documented code instead of being
/// silently accepted, the two entity toggles and the `include_exact` rendering
/// toggle through their documented defaults, and the temporal-window
/// `query_anchor` passed through unchanged (the arm and its validation belong to
/// `clio_retrieve`). The rendering itself is produced and capped in
/// `clio_retrieve`; this binds only the request control. `compose_context` is a
/// different tool and takes its own controls. The engine clock is never read
/// from the call: the published schema declares no timestamp argument, so the
/// request carries no `now` and the engine keeps its system-clock fallback.
///
/// Co-activation handoff failures surface as `coactivation_handoff_failed`,
/// which the protocol layer maps to JSON-RPC `-32001`.
#[inline(never)]
fn retrieve(state: &McpState, args: &Value) -> Result<Value, AmError> {
    retrieve_at(state, args, None)
}

/// The `retrieve` binding with the engine clock supplied by the caller: `now`
/// becomes `RetrieveRequest.now`, the fixed clock the retention curve reads,
/// while every other control binds exactly as the published tool does.
fn retrieve_at(state: &McpState, args: &Value, now: Option<String>) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let (recall_scope, duplicate_tolerance_read) = read_policy(state, &ctx, args)?;
    let (retention_curve, retention_floor) = read_curve(state, &ctx);
    let provenance_floor =
        read_provenance_floor(state, ctx.bank.as_str(), args.get("provenance_floor"))?;
    let request = RetrieveRequest {
        bank: ctx.bank.as_str().to_owned(),
        query: req_str(args, "query")?.to_owned(),
        domains: str_list(args, "domains"),
        limit: opt_usize(args, "limit").unwrap_or(DEFAULT_LIMIT),
        as_of: opt_str(args, "as_of").map(str::to_owned),
        time_axis: time_axis(args)?,
        expand_graph: opt_bool(args, "expand_graph", false),
        budget_tokens: opt_usize(args, "budget_tokens"),
        now,
        query_anchor: opt_str(args, "query_anchor").map(str::to_owned),
        recall_scope,
        prefer_consolidated: opt_bool(args, "prefer_consolidated", false),
        duplicate_tolerance_read,
        retention_curve,
        retention_floor,
        min_scores: min_scores_from_value(args.get("min_scores"))?,
        include_entities: opt_bool(args, "include_entities", true),
        entity_summary: opt_bool(args, "entity_summary", false),
        include_exact: opt_bool(args, "include_exact", false),
        provenance_floor,
    };
    let outcome = state.retriever.retrieve(&request)?;
    let mut value = serde_json::to_value(&outcome).expect("serialize retrieve outcome");
    if opt_bool(args, "explain", false) {
        value["explanation"] = retrieve_explanation(&outcome, &request.domains);
    }
    Ok(value)
}

/// Resolve the read-time scope/dedup policy: an explicit `recall_scope` call
/// argument wins, otherwise the bank's `recall_scope_default` applies. The
/// read-time tolerance always comes from the bank profile (its own mapping).
pub(crate) fn read_policy(
    state: &McpState,
    ctx: &OpContext,
    args: &Value,
) -> Result<(RecallScope, DuplicateTolerance), AmError> {
    let (profile, _) = state.retention_effective(ctx.bank.as_str());
    let scope = match opt_str(args, "recall_scope") {
        Some(raw) => RecallScope::parse(raw)?,
        None => profile.recall_scope_default,
    };
    Ok((scope, profile.duplicate_tolerance_read))
}

/// Resolve the read-time retention curve policy for the bank: the active flag
/// and the floor. A missing or unreadable profile resolves to the builtin
/// default, exactly as the rest of the read-side policy does.
pub(crate) fn read_curve(state: &McpState, ctx: &OpContext) -> (bool, f64) {
    let (profile, _) = state.retention_effective(ctx.bank.as_str());
    (profile.retention_curve, profile.retention_floor)
}

impl ReadPolicyResolver for McpState {
    /// The same effective retention profile the read handlers resolve (bank
    /// override, deployment default, builtin), so an in-process caller of the
    /// `clio-retrieve` JSON surface binds the profile this runtime reads.
    fn read_policy(&self, bank: &str) -> RetentionProfile {
        self.retention_effective(bank).0
    }
}

/// PII-safe retrieval trace: ids, ranks, scores, counts, timings only.
///
/// Per-hit fields come from the shared projection in [`crate::read_explain`],
/// which owns the explicit content-exclusion mask; the top-level counters are
/// assembled here. An inert dedup report is omitted and the `temporal_candidates`
/// counter appears only when the temporal arm ran, so a plain call's trace keeps
/// the pre-scope shape.
#[inline(never)]
fn retrieve_explanation(outcome: &RetrieveOutcome, domains: &[String]) -> Value {
    let hits: Vec<Value> = outcome
        .hits
        .iter()
        .map(|h| serde_json::to_value(explain_hit(h)).expect("serialize explain hit"))
        .collect();
    let mut explanation = json!({
        "domains": domains,
        "dense_candidates": outcome.dense_candidates,
        "lexical_candidates": outcome.lexical_candidates,
        "fused_candidates": outcome.fused_candidates,
        "reranked": outcome.reranked,
        "graph_expanded": outcome.graph_expanded,
        "pairs_reinforced": outcome.pairs_reinforced,
        "metrics": outcome.metrics,
        "warnings": outcome.warnings,
        "hits": hits
    });
    if !outcome.dedup.is_inert() {
        explanation["dedup"] =
            serde_json::to_value(&outcome.dedup).expect("serialize dedup report");
    }
    if let Some(candidates) = outcome.temporal_candidates {
        explanation["temporal_candidates"] = json!(candidates);
    }
    explanation
}

/// `compose_context(query?, domains?, budget_tokens)`: bounded injection pack.
///
/// The persona channel is assembled from `persona_get` under the persona budget
/// and always emitted; the memory section stays inside the remaining budget.
/// Explicit calls never consult the gate. The `fidelity` argument picks which
/// half each memory hit contributes and is resolved by
/// [`read_fidelity`](crate::read_read_policy::read_fidelity); which form a hit
/// then takes and how it is marked are `clio-retrieve`'s. As with `retrieve`,
/// the published schema declares no timestamp argument, so the engine clock is
/// never read from the call; the persona budget keeps its own pre-phase clock
/// resolution.
#[inline(never)]
fn compose_context(state: &McpState, args: &Value) -> Result<Value, AmError> {
    compose_context_at(state, args, None)
}

/// The `compose_context` binding with the engine clock supplied by the caller:
/// `now` becomes `ComposeRequest.now`, the fixed clock the memory section's
/// retention curve reads, while every other control binds exactly as the
/// published tool does.
fn compose_context_at(
    state: &McpState,
    args: &Value,
    now: Option<String>,
) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let timestamp = state.resolve_timestamp(args);
    let (recall_scope, duplicate_tolerance_read) = read_policy(state, &ctx, args)?;
    let (retention_curve, retention_floor) = read_curve(state, &ctx);
    // No raw argument: `compose_context` resolves the bank default for its own
    // retrieval and publishes no floor parameter of its own.
    let provenance_floor = read_provenance_floor(state, ctx.bank.as_str(), None)?;
    let fidelity = read_fidelity(state, ctx.bank.as_str(), args.get("fidelity"))?;
    let request = ComposeRequest {
        bank: ctx.bank.as_str().to_owned(),
        query: opt_str(args, "query").map(str::to_owned),
        domains: str_list(args, "domains"),
        budget_tokens: opt_usize(args, "budget_tokens").unwrap_or(DEFAULT_BUDGET_TOKENS),
        persona_entries: persona_entries(state, &ctx, &timestamp)?,
        as_of: opt_str(args, "as_of").map(str::to_owned),
        time_axis: time_axis(args)?,
        expand_graph: opt_bool(args, "expand_graph", false),
        now,
        query_anchor: opt_str(args, "query_anchor").map(str::to_owned),
        recall_scope,
        prefer_consolidated: opt_bool(args, "prefer_consolidated", false),
        duplicate_tolerance_read,
        retention_curve,
        retention_floor,
        provenance_floor,
        fidelity,
    };
    let composer = Composer::new(std::sync::Arc::clone(&state.retriever));
    let pack = composer.compose(&request, None)?;
    let mut value = serde_json::to_value(&pack).expect("serialize compose pack");
    if opt_bool(args, "explain", false) {
        value["explanation"] = compose_explanation(&pack);
    }
    Ok(value)
}

/// Persona entries assembled under the dedicated persona budget.
#[inline(never)]
fn persona_entries(
    state: &McpState,
    ctx: &OpContext,
    timestamp: &str,
) -> Result<Vec<PersonaEntry>, AmError> {
    let outcome = clio_persona::persona_get(&clio_persona::PersonaGetArgs {
        store: &*state.persona,
        ctx,
        budget_tokens: clio_persona::DEFAULT_PERSONA_TOKEN_BUDGET,
        timestamp,
    })?;
    Ok(
        clio_persona::budget::persona_pack_entries(&outcome.document)
            .into_iter()
            .map(|e| PersonaEntry {
                key: e.key,
                text: e.text,
                score: e.score,
            })
            .collect(),
    )
}

/// PII-safe compose trace: section budget accounting only.
///
/// An inert dedup report is omitted, so a plain call's trace keeps the
/// pre-scope shape.
#[inline(never)]
fn compose_explanation(pack: &ComposePack) -> Value {
    let mut explanation = json!({
        "persona_budget_tokens": pack.persona_budget_tokens,
        "memory_budget_tokens": pack.memory_budget_tokens,
        "total_tokens": pack.total_tokens,
        "retrieval_ran": pack.retrieval_ran,
        "truncated_ids": pack.truncated_ids,
        "warnings": pack.warnings
    });
    if !pack.dedup.is_inert() {
        explanation["dedup"] = serde_json::to_value(&pack.dedup).expect("serialize dedup report");
    }
    explanation
}

/// `associations(item_id, min_weight?)`: neighbor edges + effective weights.
#[inline(never)]
fn associations(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let rows = state.assoc.associations(
        ctx.bank.as_str(),
        req_str(args, "item_id")?,
        opt_num(args, "min_weight"),
    )?;
    Ok(json!({ "associations": rows }))
}

/// `graph_query(seed_id, max_hops?, edge_type?, min_weight?)`: bounded traversal.
#[inline(never)]
fn graph_query(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let hops = opt_u32(args, "max_hops").map(|h| h as usize);
    if let Some(h) = hops
        && h > MAX_GRAPH_HOPS
    {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!("graph_query max_hops must be <= {MAX_GRAPH_HOPS}"),
        ));
    }
    let edge_type = match opt_str(args, "edge_type") {
        None => None,
        Some(raw) => Some(AssocKind::parse(raw)?),
    };
    let rows = state.assoc.graph_query(
        ctx.bank.as_str(),
        req_str(args, "seed_id")?,
        hops,
        edge_type,
        opt_num(args, "min_weight"),
    )?;
    Ok(json!({ "nodes": rows }))
}

#[cfg(test)]
#[path = "read_retrieve_dedup_tests.rs"]
mod read_retrieve_dedup_tests;

#[cfg(test)]
#[path = "read_retrieve_curve_tests.rs"]
mod read_retrieve_curve_tests;

#[cfg(test)]
#[path = "read_retrieve_entities_tests.rs"]
mod read_retrieve_entities_tests;

#[cfg(test)]
#[path = "read_retrieve_entity_controls_tests.rs"]
mod read_retrieve_entity_controls_tests;

#[cfg(test)]
#[path = "read_retrieve_floors_tests.rs"]
mod read_retrieve_floors_tests;

#[cfg(test)]
#[path = "read_retrieve_provenance_tests.rs"]
mod read_retrieve_provenance_tests;

#[cfg(test)]
#[path = "read_retrieve_exact_tests.rs"]
mod read_retrieve_exact_tests;

#[cfg(test)]
#[path = "read_scores_test_support.rs"]
mod read_scores_test_support;

#[cfg(test)]
#[path = "read_retrieve_scores_age_factor_tests.rs"]
mod read_retrieve_scores_age_factor_tests;

#[cfg(test)]
#[path = "read_retrieve_scores_trust_factor_tests.rs"]
mod read_retrieve_scores_trust_factor_tests;

#[cfg(test)]
#[path = "read_retrieve_scores_tests.rs"]
mod read_retrieve_scores_tests;

#[cfg(test)]
#[path = "read_retrieve_temporal_tests.rs"]
mod read_retrieve_temporal_tests;
