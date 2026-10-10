// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the in-process JSON tool surface for
//! `intent_gate`, `retrieve`, and `compose_context`.
//!
//! ## Owns
//! - Wire argument structs and `*_from_json` handlers returning `ToolResult`
//! - The automatic-vs-explicit compose gate wiring, and passing the axis parsed
//!   by `surface_policy` into each request
//! - Parsing the optional score floors, the entity-inclusion controls, and the
//!   rendering control into the request (`floors`, `entity_summary`, `render`)
//!
//! ## Does not own
//! - Resolving the effective scope, provenance floor, and pack fidelity channel
//!   from a bank profile (`surface_policy`)
//! - Gate heuristics (`intent`), retrieval (`hybrid`), or packing (`compose`)
//! - MCP transport binding (later work)
//!
//! ## Boundary
//!
//! Changes to the JSON argument contract should normally require changes here.
//! Changes to gate/retrieval/compose internals should not require changes here.
//!
//! # Explicit vs automatic
//!
//! `retrieve_from_json` and explicit `compose_from_json` never consult the gate;
//! they run whenever a query is present. Only automatic injection honors a skip
//! decision: set `auto=true` with `retrieval_needed=false` on compose arguments.
//!
//! # Bank read policy
//!
//! Like the `bank` argument, the bank's read-time policy is injected by the
//! binding, not passed by the caller. A binding that holds retention state
//! implements [`ReadPolicyResolver`] and uses the `*_with_policy` handlers, so an
//! in-process call resolves the same effective retention profile as the MCP
//! binding: the scope, tolerance, provenance-floor, fidelity, and curve
//! defaults. Without a resolver the handlers apply the builtin default profile,
//! so `retrieve_from_json` / `compose_from_json` are explicit-parameter-only
//! entry points.
//!
//! Keep this module focused on the in-process JSON tool contract.

use crate::compose::{ComposePack, ComposeRequest, Composer, DEFAULT_BUDGET_TOKENS, PersonaEntry};
use crate::floors::MinScores;
use crate::hybrid::HybridRetriever;
use crate::intent::{GateDecision, intent_gate};
use crate::surface_policy::{invalid, parse_axis, resolve_policy};
use crate::types::{DEFAULT_LIMIT, RetrieveOutcome, RetrieveRequest};
use clio_config::ranking::IntentGateThresholds;
use clio_config::retention::RetentionProfile;
use clio_types::{AmError, ErrorCode, ToolResult};
use serde::Deserialize;
use serde_json::Value;

/// Bank read-policy seam for the in-process JSON surface.
///
/// A binding implements this with its retention profile resolution (bank
/// override, deployment default, builtin) and passes it to
/// [`retrieve_from_json_with_policy`] / [`compose_from_json_with_policy`]. The
/// resolved profile supplies the scope, tolerance, provenance-floor, fidelity,
/// and curve defaults. An explicit call parameter always wins over the resolved
/// default.
pub trait ReadPolicyResolver {
    /// Resolve the bank's effective retention profile.
    fn read_policy(&self, bank: &str) -> RetentionProfile;
}

/// Arguments for the `intent_gate` tool.
#[derive(Debug, Deserialize)]
pub struct IntentGateArgs {
    /// Turn text to classify.
    pub turn_text: String,
    /// Optional skip-confidence override (else the ranking-env default).
    #[serde(default)]
    pub skip_confidence: Option<f64>,
}

/// Arguments for the `retrieve` tool.
#[derive(Debug, Deserialize)]
pub struct RetrieveArgs {
    /// Bank isolation key, supplied by the session/transport binding rather
    /// than the caller: the published `retrieve(query, ...)` signature omits it.
    pub bank: String,
    /// Query text.
    pub query: String,
    /// Domain filters.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Result limit (default [`DEFAULT_LIMIT`]).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Point-in-time filter.
    #[serde(default)]
    pub as_of: Option<String>,
    /// `valid` (default) or `transaction`.
    #[serde(default)]
    pub time_axis: Option<String>,
    /// Explicit anchor for a relative temporal expression in the query:
    /// RFC 3339 text with an explicit UTC offset. Absent means the temporal arm
    /// does not run; there is no server-time fallback.
    #[serde(default)]
    pub query_anchor: Option<String>,
    /// Bounded graph expansion.
    #[serde(default)]
    pub expand_graph: Option<bool>,
    /// Optional pack budget.
    #[serde(default)]
    pub budget_tokens: Option<usize>,
    /// `full` (default) or `consolidated_only`.
    #[serde(default)]
    pub recall_scope: Option<String>,
    /// Drop raw items whose consolidated parent is also returned.
    #[serde(default)]
    pub prefer_consolidated: Option<bool>,
    /// Optional per-stage score floors; every field is inclusive and an absent
    /// field is a no-op. `semantic`/`keyword` prune one arm each before fusion;
    /// `reranker`/`final` filter the ranked result. See `crate::floors`.
    #[serde(default)]
    pub min_scores: Option<MinScores>,
    /// Provenance floor: `verified`, `stated`, `third_party`, or `inferred`.
    /// Absent means no floor. Any other value is rejected as
    /// `invalid_argument`, never ignored, so a mistyped floor cannot return an
    /// unfiltered page. See `crate::floors`.
    #[serde(default)]
    pub provenance_floor: Option<String>,
    /// Injectable clock for the recency boost: RFC 3339 UTC text. Absent means
    /// the server's current time, so a caller that needs a reproducible page
    /// pins it here.
    #[serde(default)]
    pub now: Option<String>,
    /// Publish each hit's `entities[]` names (default true).
    #[serde(default)]
    pub include_entities: Option<bool>,
    /// Request the top-level entity summary (default false). Rejected together
    /// with `include_entities: false`, since a summary lists entity names.
    #[serde(default)]
    pub entity_summary: Option<bool>,
    /// Attach each hit's bounded rendering (default false).
    #[serde(default)]
    pub include_exact: Option<bool>,
}

/// Arguments for the `compose_context` tool.
#[derive(Debug, Deserialize)]
pub struct ComposeArgs {
    /// Bank isolation key; same session-injection convention as [`RetrieveArgs::bank`].
    pub bank: Option<String>,
    /// Optional memory-section query.
    #[serde(default)]
    pub query: Option<String>,
    /// Domain filters for the memory section.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Total token budget (default [`DEFAULT_BUDGET_TOKENS`]).
    #[serde(default)]
    pub budget_tokens: Option<usize>,
    /// Pre-rendered persona entries.
    #[serde(default)]
    pub persona_entries: Vec<PersonaEntry>,
    /// Point-in-time filter.
    #[serde(default)]
    pub as_of: Option<String>,
    /// `valid` (default) or `transaction`.
    #[serde(default)]
    pub time_axis: Option<String>,
    /// Explicit anchor for a relative temporal expression in `query` (RFC 3339
    /// with an explicit UTC offset). Absent means the temporal arm does not run.
    #[serde(default)]
    pub query_anchor: Option<String>,
    /// Bounded graph expansion.
    #[serde(default)]
    pub expand_graph: Option<bool>,
    /// `full` (default) or `consolidated_only`.
    #[serde(default)]
    pub recall_scope: Option<String>,
    /// Drop raw items whose consolidated parent is also returned in the memory section.
    #[serde(default)]
    pub prefer_consolidated: Option<bool>,
    /// Automatic injection path (honors the gate decision).
    #[serde(default)]
    pub auto: Option<bool>,
    /// Gate decision for the automatic path.
    #[serde(default)]
    pub retrieval_needed: Option<bool>,
    /// Fidelity channel for the memory section: `flow` or `exact`. Absent
    /// means the bank's `pack_fidelity_default`.
    #[serde(default)]
    pub fidelity: Option<String>,
}

/// Dispatch `intent_gate` from JSON arguments.
#[must_use]
pub fn intent_gate_from_json(args: &Value) -> ToolResult<GateDecision> {
    let parsed: IntentGateArgs = match serde_json::from_value(args.clone()) {
        Ok(parsed) => parsed,
        Err(e) => return ToolResult::failure(invalid(format!("intent_gate args: {e}"))),
    };
    let thresholds = IntentGateThresholds {
        skip_confidence: parsed.skip_confidence,
    };
    ToolResult::success(intent_gate(&parsed.turn_text, &thresholds))
}

/// Dispatch `retrieve` from JSON arguments (explicit; never gated).
///
/// Explicit-parameter-only: the bank read policy is not resolved. Bindings use
/// [`retrieve_from_json_with_policy`] when they want the bank defaults applied.
#[must_use]
pub fn retrieve_from_json(
    retriever: &HybridRetriever,
    args: &Value,
) -> ToolResult<RetrieveOutcome> {
    retrieve_from_json_with_policy(retriever, args, None)
}

/// Dispatch `retrieve` from JSON arguments with the binding's bank read policy.
#[must_use]
pub fn retrieve_from_json_with_policy(
    retriever: &HybridRetriever,
    args: &Value,
    policy: Option<&dyn ReadPolicyResolver>,
) -> ToolResult<RetrieveOutcome> {
    let parsed: RetrieveArgs = match serde_json::from_value(args.clone()) {
        Ok(parsed) => parsed,
        Err(e) => return ToolResult::failure(invalid(format!("retrieve args: {e}"))),
    };
    let time_axis = match parse_axis(parsed.time_axis.as_deref()) {
        Ok(axis) => axis,
        Err(e) => return ToolResult::failure(e),
    };
    let (
        recall_scope,
        duplicate_tolerance_read,
        retention_curve,
        retention_floor,
        provenance_floor,
        _,
    ) = match resolve_policy(
        parsed.recall_scope.as_deref(),
        parsed.provenance_floor.as_deref(),
        None,
        &parsed.bank,
        policy,
    ) {
        Ok(policy) => policy,
        Err(e) => return ToolResult::failure(e),
    };
    let min_scores = match parsed.min_scores {
        Some(scores) => match scores.validate() {
            Ok(()) => scores,
            Err(e) => return ToolResult::failure(e),
        },
        None => MinScores::default(),
    };
    let request = RetrieveRequest {
        bank: parsed.bank,
        query: parsed.query,
        domains: parsed.domains,
        limit: parsed.limit.unwrap_or(DEFAULT_LIMIT),
        as_of: parsed.as_of,
        time_axis,
        expand_graph: parsed.expand_graph.unwrap_or(false),
        budget_tokens: parsed.budget_tokens,
        now: parsed.now,
        query_anchor: parsed.query_anchor,
        recall_scope,
        prefer_consolidated: parsed.prefer_consolidated.unwrap_or(false),
        duplicate_tolerance_read,
        retention_curve,
        retention_floor,
        min_scores,
        provenance_floor,
        include_entities: parsed.include_entities.unwrap_or(true),
        entity_summary: parsed.entity_summary.unwrap_or(false),
        include_exact: parsed.include_exact.unwrap_or(false),
    };
    ToolResult::from_result(retriever.retrieve(&request))
}

/// Dispatch `compose_context` from JSON arguments.
///
/// Pass `gate` for the automatic path; explicit callers pass `None`. Setting
/// `auto=true` in the arguments builds an internal gate from `retrieval_needed`
/// instead. Explicit-parameter-only: see [`compose_from_json_with_policy`].
#[must_use]
pub fn compose_from_json(
    composer: &Composer,
    args: &Value,
    gate: Option<&GateDecision>,
) -> ToolResult<ComposePack> {
    compose_from_json_with_policy(composer, args, gate, None)
}

/// Dispatch `compose_context` from JSON arguments with the binding's bank read policy.
#[must_use]
pub fn compose_from_json_with_policy(
    composer: &Composer,
    args: &Value,
    gate: Option<&GateDecision>,
    policy: Option<&dyn ReadPolicyResolver>,
) -> ToolResult<ComposePack> {
    let parsed: ComposeArgs = match serde_json::from_value(args.clone()) {
        Ok(parsed) => parsed,
        Err(e) => return ToolResult::failure(invalid(format!("compose_context args: {e}"))),
    };
    let Some(bank) = parsed.bank else {
        return ToolResult::failure(AmError::new(
            ErrorCode::MissingBank,
            "compose_context requires a bank",
        ));
    };
    let time_axis = match parse_axis(parsed.time_axis.as_deref()) {
        Ok(axis) => axis,
        Err(e) => return ToolResult::failure(e),
    };
    let auto_gate = GateDecision {
        retrieval_needed: parsed.retrieval_needed.unwrap_or(true),
        domains: Vec::new(),
        confidence: 0.0,
        reason: "auto".to_owned(),
    };
    let effective_gate = if parsed.auto == Some(true) {
        Some(&auto_gate)
    } else {
        gate
    };
    let (
        recall_scope,
        duplicate_tolerance_read,
        retention_curve,
        retention_floor,
        provenance_floor,
        fidelity,
    ) = match resolve_policy(
        parsed.recall_scope.as_deref(),
        None,
        parsed.fidelity.as_deref(),
        &bank,
        policy,
    ) {
        Ok(policy) => policy,
        Err(e) => return ToolResult::failure(e),
    };
    let request = ComposeRequest {
        bank,
        query: parsed.query,
        domains: parsed.domains,
        budget_tokens: parsed.budget_tokens.unwrap_or(DEFAULT_BUDGET_TOKENS),
        persona_entries: parsed.persona_entries,
        as_of: parsed.as_of,
        time_axis,
        expand_graph: parsed.expand_graph.unwrap_or(false),
        now: None,
        query_anchor: parsed.query_anchor,
        recall_scope,
        prefer_consolidated: parsed.prefer_consolidated.unwrap_or(false),
        duplicate_tolerance_read,
        retention_curve,
        retention_floor,
        provenance_floor,
        fidelity,
    };
    ToolResult::from_result(composer.compose(&request, effective_gate))
}

#[cfg(test)]
#[path = "surface_tests.rs"]
mod surface_tests;
