// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP store-path core: gated scoring and the
//! optional durable write shared by `store`, `admit_preview`, and the batch preview.
//!
//! ## Owns
//! - `store_path` scoring/write orchestration and the `admit_preview`/`store` entry points
//! - The top-level decision response shape for those tools
//!
//! ## Does not own
//! - Argument decoding into items/candidates (see `write_tools`)
//! - Admission math or span verification (domain crates own those)
//!
//! ## Boundary
//!
//! Changes to store-path orchestration or its response shape should normally require
//! changes here. Changes to tool routing or argument helpers should not require changes here.
//!
//! Keep this module focused on the store-path scoring and write core.

use crate::runtime::{McpState, StderrSinks, StoreSignals};
use crate::write_tools::{
    admission_policy, attach_leaf, build_candidate, build_item, opt_bool, opt_str, tool_result,
};
use clio_admission::is_exact_duplicate_reason;
use clio_admission::policy::AdmissionPolicy;
use clio_types::{AmError, ErrorCode, MemoryItem};
use clio_write::store_path::VerifiedStoreRequest;
use clio_write::verify::VerifyConfig;
use clio_write::{default_event_fields, gated_store_verified};
use serde_json::{Map, Value};
use std::sync::Arc;

/// The item's gist for a store-path write. An absent gist stays absent. An empty
/// or whitespace-only gist is refused, so no write path stores blank text.
pub(crate) fn gist_of(shell: &Value) -> Result<Option<String>, AmError> {
    match shell.get("gist").and_then(Value::as_str) {
        Some(text) if text.trim().is_empty() => Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item.gist must not be empty or whitespace-only",
        )),
        Some(text) if !text.is_empty() => Ok(Some(text.to_owned())),
        _ => Ok(None),
    }
}

/// An item needs content: a non-blank gist, or a snapshot. An item with neither
/// would store an empty memory, so every store-path entry refuses it.
pub(crate) fn require_content(args: &Value) -> Result<(), AmError> {
    let shell = args.get("item").unwrap_or(&Value::Null);
    let has_snapshot = shell.get("snapshot").is_some_and(|v| !v.is_null());
    if gist_of(shell)?.is_none() && !has_snapshot {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item.gist is required unless item.snapshot is given",
        ));
    }
    Ok(())
}

/// Store-path core shared by `admit_preview` (never writes), `store`, and the
/// batch preview. `retention` overrides the resolved bank profile when present.
#[inline(never)]
pub(crate) fn store_path(
    state: &McpState,
    args: &Value,
    allow_write: bool,
    retention: Option<clio_config::retention::ResolvedRetention>,
) -> Result<Value, AmError> {
    require_content(args)?;
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args)?;
    let candidate = build_candidate(args)?;
    let item_id = args
        .pointer("/item/id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map_or_else(|| state.next_item_id(), str::to_owned);
    let item = build_item(state, &ctx, args, item_id)?;
    let fields = default_event_fields();
    let policy = match retention {
        Some(resolved) => AdmissionPolicy::with_retention(&state.env, resolved),
        None => admission_policy(state, ctx.bank.as_str()),
    }
    .with_force(opt_bool(args, "allow_duplicate", false)?);
    let mut req = VerifiedStoreRequest {
        item,
        ctx: &ctx,
        source_text: opt_str(args, "source_text"),
        evidence_ref: opt_str(args, "evidence_ref"),
        fields: &fields,
        verify: VerifyConfig::default(),
        policy,
        admit_sink: &StderrSinks,
        verify_sink: &StderrSinks,
        timestamp: &ts,
    };
    let dry_run = opt_bool(args, "dry_run", false)? || !allow_write;
    let mut stored_id: Option<String> = None;
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let decision = gated_store_verified(
        &mut req,
        candidate,
        &signals,
        &mut |written: &MemoryItem| {
            if dry_run {
                return Ok(());
            }
            state
                .store
                .create_memory_item(written, ctx.actor.as_str())?;
            let signal = attach_leaf(state, written)?;
            stored_id = Some(written.id.clone());
            state.dispatch_maintenance(vec![signal]);
            Ok(())
        },
    )?;
    // The decision fields (`pass`, `admission_score`, `factors`,
    // `rejection_reason?`) are returned at the top level, alongside the stored id.
    let mut out = Map::new();
    if let Value::Object(fields) =
        serde_json::to_value(&decision).expect("serialize admission decision")
    {
        out.extend(fields);
    }
    if let Some(id) = stored_id.as_deref() {
        out.insert("id".into(), Value::from(id));
    }
    // The write reports the bank each item landed in and its id, so a one-shot
    // caller can index exactly those items without resolving the bank again. A
    // refused or dry run reports no written item.
    out.insert(
        "written".into(),
        Value::Array(
            stored_id
                .iter()
                .map(|id| {
                    Value::Object(Map::from_iter([
                        ("bank".to_owned(), Value::from(ctx.bank.as_str())),
                        ("id".to_owned(), Value::from(id.as_str())),
                    ]))
                })
                .collect(),
        ),
    );
    // An exact-duplicate rejection stored nothing, so the payload says so
    // explicitly. Hosts turn this into a no-op success rather than a failure:
    // the caller already holds the fact, and repeating it is not an error.
    if stored_id.is_none()
        && decision
            .rejection_reason
            .as_deref()
            .is_some_and(is_exact_duplicate_reason)
    {
        out.insert("duplicate".into(), Value::Bool(true));
    }
    if dry_run {
        out.insert("dry_run".into(), Value::from(true));
    }
    Ok(Value::Object(out))
}

/// `admit_preview`: identical gates, never writes.
#[inline(never)]
pub(crate) fn admit_preview(state: &McpState, args: &Value) -> Result<Value, AmError> {
    store_path(state, args, false, None)
}

/// `store`: gated write; `dry_run` skips only the durable write step.
#[inline(never)]
pub(crate) fn store(state: &McpState, args: &Value) -> Result<Value, AmError> {
    store_path(state, args, true, None)
}

/// The `store` tool envelope. A write the gates refused (an exact duplicate, or
/// any other admission rejection) stored nothing, so it returns `ok:false` with
/// the same code the CLI uses. A caller that checks `ok` never reads a refused
/// write as stored. Previews and dry runs keep their verdict under `ok:true`.
#[inline(never)]
pub(crate) fn store_envelope(state: &McpState, args: &Value) -> Value {
    match store(state, args) {
        Ok(value) => match refusal_of(&value) {
            Some((code, message)) => refused_envelope(value, code, &message),
            None => tool_result(Ok(value)),
        },
        Err(err) => tool_result(Err(err)),
    }
}

/// The refusal code and message for a write the gates turned away. `None` for a
/// stored item, a preview, or a dry run.
fn refusal_of(value: &Value) -> Option<(&'static str, String)> {
    let refused = value.get("pass").and_then(Value::as_bool) == Some(false);
    if !refused || value.get("id").is_some() || value.get("dry_run").is_some() {
        return None;
    }
    let reason = value
        .get("rejection_reason")
        .and_then(Value::as_str)
        .unwrap_or("write refused by the admission gates");
    if value.get("duplicate") == Some(&Value::Bool(true)) {
        return Some(("duplicate", format!("duplicate: nothing stored ({reason})")));
    }
    Some(("admission_refused", reason.to_owned()))
}

/// Re-shape a refused write as `ok:false`. The decision fields stay, so the
/// reason and the score remain visible to the caller.
fn refused_envelope(mut value: Value, code: &str, message: &str) -> Value {
    if let Value::Object(fields) = &mut value {
        fields.insert("ok".into(), Value::Bool(false));
        fields.insert("code".into(), Value::from(code));
        fields.insert("message".into(), Value::from(message));
    }
    value
}

#[cfg(test)]
#[path = "store_write_tests.rs"]
mod tests;
