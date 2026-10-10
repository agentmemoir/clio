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
use clio_types::{AmError, ErrorCode};
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

/// Run the in-process surface, returning the serialized report/outcome.
fn ops_result(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    let timestamp = state.resolve_timestamp(args);
    match name {
        "diagnose" => {
            let report = clio_ops::diagnose(&clio_ops::DiagnoseArgs {
                store: &*state.ops_store,
                index: &*state.index,
                embedder: embedder(state),
                timestamp: &timestamp,
            });
            Ok(to_value(&report))
        }
        "verify" => {
            let report = clio_ops::verify(&clio_ops::VerifyArgs {
                store: &*state.ops_store,
                index: &*state.index,
                bank: opt_str(args, "bank"),
                timestamp: &timestamp,
            })?;
            Ok(to_value(&report))
        }
        "doctor" => {
            let plan = clio_ops::doctor(&clio_ops::DoctorArgs {
                store: &*state.ops_store,
                index: &*state.index,
                bank: opt_str(args, "bank"),
                dry_run: opt_bool(args, "dry_run", true),
                timestamp: &timestamp,
                plans: &state.ops_plans,
            })?;
            Ok(to_value(&plan))
        }
        "repair" => {
            // Mutating: enforce the same bank authorization as other tools
            // (shared namespace requires the deployment to enable it).
            state.resolve_ctx(args)?;
            let actions = string_array(args, "actions")?;
            let outcome = clio_ops::repair(&clio_ops::RepairArgs {
                store: &*state.ops_store,
                writer: &*state.store,
                index: &*state.index,
                embedder: embedder(state),
                plans: &state.ops_plans,
                plan_id: opt_str(args, "plan_id"),
                actions: &actions,
                bank: opt_str(args, "bank"),
                confirm: opt_bool(args, "confirm", false),
                timestamp: &timestamp,
            })?;
            Ok(to_value(&outcome))
        }
        "reindex" => {
            // Mutating: same bank authorization gate as `repair`.
            state.resolve_ctx(args)?;
            // Whole-store reindex must not silently expand into banks the
            // caller cannot authorize: when the shared bank is disabled, a
            // bank-less scope could still sweep `shared` via indexable_banks.
            let bank = match opt_str(args, "bank") {
                Some(b) => Some(b),
                None if state.shared_enabled => None,
                None => {
                    return Err(AmError::new(
                        ErrorCode::Forbidden,
                        "reindex without `bank` sweeps every indexable bank; pass an \
                         explicit bank while the shared bank surface is disabled",
                    ));
                }
            };
            let target =
                clio_index::RebuildTargets::parse(opt_str(args, "target").unwrap_or("all"))?;
            let report = clio_ops::reindex(&clio_ops::ReindexArgs {
                store: &*state.store,
                index: &*state.index,
                embedder: embedder(state),
                bank,
                target,
                dry_run: opt_bool(args, "dry_run", true),
                confirm: opt_bool(args, "confirm", false),
            })?;
            Ok(to_value(&report))
        }
        other => Err(AmError::not_implemented(other)),
    }
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
#[path = "ops_tools_tests.rs"]
mod ops_tools_tests;
