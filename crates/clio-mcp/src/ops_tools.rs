// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP bindings for the five operator
//! surfaces: `diagnose`, `verify`, `doctor`,
//! `repair`, and `reindex`.
//!
//! ## Owns
//! - Tool-name detection and dispatch for the operator surfaces
//! - The one operator-scope gate every arm resolves its bank through
//! - JSON argument decoding into the frozen `clio-ops` argument structs
//! - The top-level `{ok, ...}` result envelope and its `{ok:false, code, ...}`
//!   failure shape
//!
//! ## Does not own
//! - Check, plan, repair, or rebuild semantics (see `clio-ops`)
//! - Bound-tool registration or schema publication (`lib` / `schema_ops_defs`)
//! - The dispatch entry point that routes here (`write_tools`)
//!
//! ## Boundary
//!
//! Changes to MCP argument decoding or the ops result envelope should normally
//! require changes here. Changes to operator check logic should not require
//! changes here.
//!
//! Keep this module focused on MCP operator-tool dispatch.

use crate::runtime::McpState;
use crate::write_tools::{opt_bool, opt_str};
use clio_types::{AmError, ErrorCode, OpContext};
use serde::Serialize;
use serde_json::{Map, Value, json};

/// True when `name` is one of the five operator surfaces.
#[must_use]
pub(crate) fn is_ops_tool(name: &str) -> bool {
    matches!(
        name,
        "diagnose" | "verify" | "doctor" | "repair" | "reindex"
    )
}

/// Route one operator tool to its `clio-ops` handler and shape the envelope.
///
/// The returned value already carries a top-level `ok`; the protocol layer
/// derives `isError` from it, and `write_tools::dispatch` returns this value
/// directly (no [`crate::write_tools::tool_result`] wrapping).
#[must_use]
pub(crate) fn dispatch_ops(state: &McpState, name: &str, args: &Value) -> Value {
    match ops_result(state, name, args) {
        Ok(value) => success_envelope(value),
        Err(err) => error_envelope(args, &err),
    }
}

/// The operator scope one call may reach: one granted bank, or the whole store.
enum OpScope<'a> {
    /// Exactly one bank; a bound credential covers it.
    Bank(&'a str),
    /// Every bank in the store; only an unbound local caller reaches it.
    WholeStore,
}

impl<'a> OpScope<'a> {
    /// The scope in the `Option<&str>` shape the `clio-ops` args take.
    fn bank(&self) -> Option<&'a str> {
        match *self {
            Self::Bank(bank) => Some(bank),
            Self::WholeStore => None,
        }
    }
}

impl McpState {
    /// The operator scope this call may reach. A bound caller is limited to the
    /// banks its credential covers and never reaches the whole store.
    ///
    /// The gate runs for every caller before the principal is consulted, so a
    /// local caller without a credential is allowed here rather than beside it.
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`] when a bound credential's list omits the scope.
    fn op_scope<'a>(
        &'a self,
        ctx: &'a OpContext,
        target: Option<&'a str>,
    ) -> Result<OpScope<'a>, AmError> {
        let bank = target.unwrap_or(ctx.bank.as_str());
        self.require_bank_grant(bank)?;
        if self.principal.is_none() {
            return Ok(target.map_or(OpScope::WholeStore, OpScope::Bank));
        }
        Ok(OpScope::Bank(bank))
    }
}

/// Run the in-process surface, returning the serialized report/outcome.
///
/// One gate runs for every operator tool: the call resolves its context, and
/// each arm then asks `McpState::op_scope` for the scope it may reach.
fn ops_result(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let timestamp = state.resolve_timestamp(args)?;
    match name {
        "diagnose" => diagnose_result(state, &ctx, &timestamp),
        "verify" => verify_result(state, args, &ctx, &timestamp),
        "doctor" => doctor_result(state, args, &ctx, &timestamp),
        "repair" => repair_result(state, args, &ctx, &timestamp),
        "reindex" => reindex_result(state, args, &ctx),
        other => Err(AmError::not_implemented(other)),
    }
}

/// The `diagnose` arm: a bound caller reads its own bank, a local one the store.
///
/// `diagnose` published no bank scope before the grant existed, so a local
/// caller without a credential keeps the whole-store snapshot whatever `bank`
/// key it carries. Only a bound principal narrows the report, to the bank its
/// grant already covers.
fn diagnose_result(state: &McpState, ctx: &OpContext, timestamp: &str) -> Result<Value, AmError> {
    let scope = state.op_scope(ctx, None)?.bank();
    let report = clio_ops::diagnose_scoped(
        &clio_ops::DiagnoseArgs {
            store: &*state.ops_store,
            index: &*state.index,
            embedder: embedder(state),
            timestamp,
        },
        scope,
    );
    Ok(to_value(&report))
}

/// The `verify` arm: the explicit `bank` argument is the target.
///
/// An omitted argument ends where the context resolved, which for a bound
/// caller is its grant bank, so the gate sees the bank `resolve_ctx` authorized.
fn verify_result(
    state: &McpState,
    args: &Value,
    ctx: &OpContext,
    timestamp: &str,
) -> Result<Value, AmError> {
    let bank = state.op_scope(ctx, opt_str(args, "bank"))?.bank();
    if let Some(bank) = bank {
        state.require_known_bank(bank)?;
    }
    let report = clio_ops::verify(&clio_ops::VerifyArgs {
        store: &*state.ops_store,
        index: &*state.index,
        bank,
        timestamp,
    })?;
    Ok(to_value(&report))
}

/// The `doctor` arm: compose a plan inside the caller's authorized scope.
///
/// The store-wide note probe reads every other bank, so it runs only for an
/// unbound local operator: a bound caller whose grant covers one bank must not
/// learn from `notes` that another bank exists or which codes it carries.
fn doctor_result(
    state: &McpState,
    args: &Value,
    ctx: &OpContext,
    timestamp: &str,
) -> Result<Value, AmError> {
    let bank = state.op_scope(ctx, opt_str(args, "bank"))?.bank();
    if let Some(bank) = bank {
        state.require_known_bank(bank)?;
    }
    let plan = clio_ops::doctor_scoped(
        &clio_ops::DoctorArgs {
            store: &*state.ops_store,
            index: &*state.index,
            bank,
            dry_run: opt_bool(args, "dry_run", true)?,
            timestamp,
            plans: &state.ops_plans,
        },
        state.principal.is_none(),
    )?;
    Ok(to_value(&plan))
}

/// The `repair` arm: resolve the plan's scope, gate it, then execute.
///
/// A registered plan names its own bank, and a bank-less request would
/// otherwise let it reach any bank in the store: resolve the plan's scope before
/// any action runs. Only a bound caller derives its scope from the plan; an
/// unbound caller keeps its own argument, and an omitted one for it stays
/// whole-store as before.
///
/// A plan registered for a bank outside the bound caller's grant is neither
/// authorized nor disclosed: it answers exactly like an id that was never
/// registered, because plan ids are sequential and a distinguishable refusal
/// would tell the caller that another bank exists.
fn repair_result(
    state: &McpState,
    args: &Value,
    ctx: &OpContext,
    timestamp: &str,
) -> Result<Value, AmError> {
    let actions = string_array(args, "actions")?;
    let explicit_bank = opt_str(args, "bank");
    // A plan the grant excludes stays unreachable whatever `bank` the request
    // also carries: an explicit, granted argument must not turn a foreign plan
    // into one the registry below resolves.
    let unreachable_plan = plan_outside_grant(state, args);
    let plan_bank = if state.principal.is_some() && !unreachable_plan {
        opt_str(args, "plan_id")
            .and_then(|id| state.ops_plans.get(id))
            .and_then(|plan| plan.bank)
    } else {
        None
    };
    let scope = state.op_scope(ctx, explicit_bank.or(plan_bank.as_deref()))?;
    // An unreachable plan is resolved against a registry that cannot hold it, so
    // the operator refuses it with the same `plan_not_found` finding, the same
    // scope, and the same empty `results` an unknown id produces, and runs no
    // action a plan could have carried.
    let unseen;
    let plans: &clio_ops::PlanRegistry = if unreachable_plan {
        unseen = clio_ops::PlanRegistry::new();
        &unseen
    } else {
        &state.ops_plans
    };
    let outcome = clio_ops::repair(&clio_ops::RepairArgs {
        store: &*state.ops_store,
        writer: &*state.store,
        index: &*state.index,
        embedder: embedder(state),
        plans,
        plan_id: opt_str(args, "plan_id"),
        actions: &actions,
        bank: scope.bank(),
        confirm: opt_bool(args, "confirm", false)?,
        timestamp,
    })?;
    Ok(to_value(&outcome))
}

/// True when the request's plan id names a registered plan the grant excludes.
///
/// The grant's own list answers, asked as a question rather than built into a
/// refusal. An id that was never registered is not outside anything, and neither
/// is a plan that names no bank: both keep the scope the context already
/// authorized. An unbound caller is outside nothing.
fn plan_outside_grant(state: &McpState, args: &Value) -> bool {
    let Some(principal) = &state.principal else {
        return false;
    };
    opt_str(args, "plan_id")
        .and_then(|id| state.ops_plans.get(id))
        .and_then(|plan| plan.bank)
        .is_some_and(|bank| !principal.covers_bank(&bank))
}

/// The `reindex` arm: one bank, or every indexable bank for a local caller.
///
/// A bound caller's omitted scope never expands to every indexable bank: the
/// resolved, authorized bank is the whole scope it gets. An unbound omitted
/// scope is still the whole store, which needs the shared surface enabled.
fn reindex_result(state: &McpState, args: &Value, ctx: &OpContext) -> Result<Value, AmError> {
    let bank = state.op_scope(ctx, opt_str(args, "bank"))?.bank();
    if bank.is_none() && !state.shared_enabled {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "reindex without `bank` sweeps every indexable bank; pass an explicit bank \
             while the shared bank surface is disabled",
        ));
    }
    let target = clio_index::RebuildTargets::parse(opt_str(args, "target").unwrap_or("all"))?;
    let report = clio_ops::reindex(&clio_ops::ReindexArgs {
        store: &*state.store,
        index: &*state.index,
        embedder: embedder(state),
        bank,
        target,
        dry_run: opt_bool(args, "dry_run", true)?,
        confirm: opt_bool(args, "confirm", false)?,
    })?;
    Ok(to_value(&report))
}

/// Merge a serialized report into a fresh map with a leading `ok: true`.
///
/// The report's own `ok` field, when present, is authoritative: a report that
/// carries `ok:false` (a `verify` failure or a `repair` policy refusal) keeps
/// `ok:false` and its `findings`.
fn success_envelope(value: Value) -> Value {
    let mut out = Map::new();
    out.insert("ok".into(), Value::Bool(true));
    if let Value::Object(fields) = value {
        for (key, val) in fields {
            out.insert(key, val);
        }
    }
    Value::Object(out)
}

/// Structured failure envelope: `{ok:false, code, message, findings}`.
///
/// Messages flow through the shared ops redactor: driver errors can
/// embed connection URLs, and the MCP surface must hide credentials exactly
/// like the CLI does.
fn error_envelope(args: &Value, err: &AmError) -> Value {
    let scope = clio_ops::scope_label(opt_str(args, "bank"));
    let message = clio_ops::redact_credentials(&err.message);
    json!({
        "ok": false,
        "code": err.code.as_str(),
        "message": message,
        "findings": [{
            "code": err.code.as_str(),
            "severity": "error",
            "scope": scope,
            "message": message,
        }]
    })
}

/// Optional embedding sidecar borrowed from the shared runtime state.
fn embedder(state: &McpState) -> Option<&dyn clio_index::Embedder> {
    state.ops_embedder.as_deref()
}

/// Decode an optional array-of-strings argument (absent/null = empty).
fn string_array(args: &Value, key: &str) -> Result<Vec<String>, AmError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str().map(str::to_owned).ok_or_else(|| {
                    AmError::new(
                        ErrorCode::InvalidArgument,
                        format!("{key} entries must be strings"),
                    )
                })
            })
            .collect(),
        Some(_) => Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("{key} must be an array of strings"),
        )),
    }
}

/// Serialize an operator report; the shapes are plain owned structs.
fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("operator report serializes")
}

#[cfg(test)]
#[path = "ops_tools_grant_tests.rs"]
mod ops_tools_grant_tests;

#[cfg(test)]
#[path = "ops_tools_grant_scope_tests.rs"]
mod ops_tools_grant_scope_tests;

#[cfg(test)]
#[path = "ops_tools_tests.rs"]
mod ops_tools_tests;
