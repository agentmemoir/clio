// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP-bound item-lifecycle mutators:
//! `update`, `invalidate`, `discard`, `correct`, `erase_request`, and
//! `consolidate`, decoded from MCP args into in-process calls.
//!
//! ## Owns
//! - JSON argument decoding and result shaping for these mutators
//! - Confirmation handling for `discard` (dry-run without `confirm`)
//!
//! ## Does not own
//! - Gated write-path handlers (see `write_tools`)
//! - Persona and task/failure record handlers (see `mutator_record_tools`)
//! - Mutator semantics; the in-process tools own behavior
//!
//! ## Boundary
//!
//! Changes to MCP argument decoding or result shaping should normally require
//! changes here. Changes to mutator internals should not require changes here.
//!
//! Keep this module focused on MCP item-lifecycle mutator dispatch.

use crate::runtime::{McpState, StderrSinks};
use crate::write_tools::{end_outcome_json, opt_bool, opt_context, opt_str, req_str};
use clio_types::{AmError, ErrorCode, UpdateRule};
use serde_json::{Value, json};

/// Route a mutator/record tool name to its in-process handler (raw result).
#[inline(never)]
pub(crate) fn dispatch_inner(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    match name {
        "update" => update(state, args),
        "invalidate" => invalidate(state, args),
        "discard" => discard(state, args),
        "correct" => correct(state, args),
        "consolidate" => consolidate(state, args),
        "erase_request" => erase_request(state, args),
        "persona_put_stable" => crate::mutator_record_tools::persona_put_stable(state, args),
        "persona_observe_preference" => crate::mutator_record_tools::persona_observe(state, args),
        "task_upsert" => crate::mutator_record_tools::task_upsert(state, args),
        "failure_record" => crate::mutator_record_tools::failure_record(state, args),
        other => Err(AmError::not_implemented(other)),
    }
}

/// Shared `update` facade: discrete invalidation vs continuous EMA.
#[inline(never)]
fn update(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let rule_raw = req_str(args, "update_rule")?;
    let new_value = args
        .get("new_value")
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        })
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "new_value is required"))?;
    let u_args = clio_write::UpdateArgs {
        store: &*state.store,
        continuous: &*state.continuous,
        ctx: &ctx,
        target: req_str(args, "target")?,
        new_value: &new_value,
        update_rule: UpdateRule::parse(rule_raw)?,
        timestamp: &ts,
        continuous_sink: &StderrSinks,
        triple_sink: &StderrSinks,
    };
    match clio_write::update(&u_args)? {
        clio_write::UpdateOutcome::Continuous(state) => Ok(
            json!({"rule": "continuous", "state": serde_json::to_value(&state).expect("serialize tool payload")}),
        ),
        clio_write::UpdateOutcome::Discrete(end) => Ok(json!({
            "rule": "discrete",
            "closed_ids": end.closed_ids,
            "already_ended": end.already_ended,
            "replacement_id": end.replacement_id
        })),
    }
}

/// `invalidate`: close validity on both axes; never deletes.
#[inline(never)]
fn invalidate(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let outcome = clio_write::triple::invalidate(
        &*state.store,
        &ctx,
        req_str(args, "item_id")?,
        opt_str(args, "replacement_id"),
        &ts,
        None,
        &StderrSinks,
    )?;
    Ok(end_outcome_json(&outcome))
}

/// `discard`: dry-run without `confirm` (harness confirmation).
#[inline(never)]
fn discard(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let confirm = opt_bool(args, "confirm", false);
    let d_args = clio_write::DiscardArgs {
        store: &*state.store,
        ctx: &ctx,
        item_id: req_str(args, "item_id")?,
        reason: req_str(args, "reason")?,
        confirm,
        timestamp: &ts,
        sink: &StderrSinks,
    };
    match clio_write::discard(&d_args)? {
        clio_write::DiscardOutcome::DryRun(identity) => Ok(json!({
            "confirm": false,
            "would_discard": identity_values(&identity)
        })),
        clio_write::DiscardOutcome::Discarded(identity) => Ok(json!({
            "confirm": true,
            "discarded": identity_values(&identity)
        })),
    }
}

/// Render a bank-scoped identity without decrypting content.
fn identity_values(identity: &clio_store::ItemIdentity) -> Value {
    json!({
        "item_id": identity.item_id,
        "category": identity.category,
        "epistemic_kind": identity.epistemic_kind
    })
}

/// True when the request carries a non-empty `new_value` (a non-empty string or
/// any non-null structured value). Belief `correct` requests must not carry one.
fn has_nonempty_new_value(args: &Value) -> bool {
    match args.get("new_value") {
        Some(Value::String(s)) => !s.is_empty(),
        Some(value) => !value.is_null(),
        None => false,
    }
}

/// `correct(item_id, new_value, reason)`: auditable correction.
///
/// Without `confirm` this resolves the correction and returns a would-correct
/// preview with zero writes (harness confirmation); with `confirm` the
/// discrete or continuous path runs with the mandatory reason recorded on the
/// same-transaction telemetry rows.
#[inline(never)]
fn correct(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let confirm = opt_bool(args, "confirm", false);
    let item_id = req_str(args, "item_id")?.to_owned();
    let reason = req_str(args, "reason")?.to_owned();
    // The blank-reason rejection must hold on the zero-write preview as well as
    // the confirmed path (security acceptance: 400-class without a reason).
    if reason.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "reason is required and must be non-empty",
        ));
    }
    // Absent/null context preserves the prior revision's context; a supplied
    // value is an audited replacement. The preview echoes presence only.
    let context = opt_context(args, "context")?;
    // Belief targets are not items: a resolvable belief routes to the
    // authorized context-only correction. Values and confidences stay
    // append-only via `belief_observe`.
    let belief = state
        .belief
        .get_belief(ctx.bank.as_str(), &item_id)?
        .or_else(|| {
            state
                .belief
                .find_belief_by_proposition(
                    ctx.bank.as_str(),
                    &clio_types::proposition_key(&item_id),
                )
                .ok()
                .flatten()
        });
    if let Some(belief) = belief {
        // A belief's value and confidence evolve only through `belief_observe`;
        // `correct` touches the item-level context only. A request that also
        // carries a value is rejected rather than silently dropping it
        // on the preview as well as the confirmed path.
        if has_nonempty_new_value(args) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "a belief's value and confidence evolve only via belief_observe; \
                 `correct` changes the context only — omit new_value to correct \
                 the belief context",
            ));
        }
        if !confirm {
            return Ok(json!({
                "confirm": false,
                "would_correct": {
                    "belief_id": belief.id,
                    "rule": "belief_context",
                    "reason": reason,
                    "context_present": context.is_some()
                }
            }));
        }
        let new_context = context.as_deref().ok_or_else(|| {
            AmError::new(
                ErrorCode::InvalidArgument,
                "context is required to correct a belief context (values and \
                 confidences evolve only via belief_observe)",
            )
        })?;
        let result =
            clio_compliance::correct_belief_context(&clio_compliance::BeliefContextCorrectArgs {
                beliefs: state.belief_corrector.as_ref(),
                bank_id: ctx.bank.as_str(),
                belief_id: &belief.id,
                new_context,
                reason: &reason,
                actor: ctx.actor.as_str(),
            })?;
        return Ok(serde_json::to_value(&result).expect("serialize tool payload"));
    }
    // `new_value` may be a string or a structured JSON value; structured
    // values are serialized so the in-process layer can re-parse them.
    let new_value = match args.get("new_value") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(v) if !v.is_null() => serde_json::to_string(v).expect("serialize new_value"),
        _ => {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "new_value is required",
            ));
        }
    };
    if !confirm {
        return Ok(json!({
            "confirm": false,
            "would_correct": {
                "item_id": item_id,
                "new_value": new_value,
                "reason": reason,
                "context_present": context.is_some()
            }
        }));
    }
    let c_args = clio_compliance::CorrectArgs {
        store: &*state.store,
        continuous: &*state.continuous,
        bank_id: ctx.bank.as_str(),
        actor: ctx.actor.as_str(),
        item_id: &item_id,
        new_value: &new_value,
        reason: &reason,
        context: context.as_deref(),
        timestamp: &ts,
        new_item_id: &state.next_item_id(),
    };
    let result = clio_compliance::correct(&c_args)?;
    Ok(serde_json::to_value(&result).expect("serialize tool payload"))
}

/// `erase_request`: verified compliance crypto-shred (never aliases discard).
///
/// Requires a privileged actor and `confirm=true`; the authorization and
/// confirmation gates live in the in-process `clio-compliance` handler.
#[inline(never)]
fn erase_request(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let salt = clio_compliance::tombstone_salt();
    let e_args = clio_compliance::EraseArgs {
        store: &*state.compliance,
        maint: Some(&state.maint),
        ctx: &ctx,
        subject_id: req_str(args, "subject_id")?,
        legal_basis: req_str(args, "legal_basis")?,
        request_id: req_str(args, "request_id")?,
        reason_code: opt_str(args, "reason_code"),
        confirm: opt_bool(args, "confirm", false),
        salt: &salt,
        timestamp: &ts,
        sink: &StderrSinks,
    };
    let outcome = clio_compliance::erase_request(&e_args)?;
    Ok(serde_json::to_value(&outcome).expect("serialize tool payload"))
}

/// `consolidate`: maintenance trigger (dirty refresh + hub distill).
#[inline(never)]
fn consolidate(state: &McpState, args: &Value) -> Result<Value, AmError> {
    // The bank selector was renamed from `scope` to `bank` with no alias. A
    // conforming client already rejects the old name against the published
    // schema; this guard covers every other caller, because a selector that
    // is silently dropped would fall back to the context bank.
    if args.get("scope").is_some() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "`consolidate` no longer accepts `scope`; pass the bank selector as `bank`",
        ));
    }
    let ctx = state.resolve_ctx(args)?;
    // `resolve_ctx` already applied args-over-default for `bank`, so the
    // resolved context bank is the wave's target bank.
    let bank = ctx.bank.as_str().to_owned();
    let force = opt_bool(args, "force", false);
    // Take the process's current ranking environment as the wave's policy, so a
    // `ranking_env_set` earlier in this process governs the wave this call
    // starts. Both bindings call this one function, so neither can drift.
    state.apply_ranking_policy();
    let result = clio_write::consolidate_tool(
        &state.maint,
        state.hub.as_ref(),
        Some(&ctx),
        &bank,
        state.resolved_retention(&bank),
        force,
        state.join_consolidate_wave,
    );
    Ok(serde_json::to_value(&result).expect("serialize tool payload"))
}

#[cfg(test)]
#[path = "mutator_tools_audit_tests.rs"]
mod mutator_tools_audit_tests;

#[cfg(test)]
#[path = "mutator_tools_tests.rs"]
mod mutator_tools_tests;
