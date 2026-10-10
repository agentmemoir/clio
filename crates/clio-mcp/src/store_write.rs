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
    admission_policy, attach_leaf, build_candidate, build_item, opt_bool, opt_str,
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

/// Store-path core shared by `admit_preview` (never writes), `store`, and the
/// batch preview. `retention` overrides the resolved bank profile when present.
#[inline(never)]
pub(crate) fn store_path(
    state: &McpState,
    args: &Value,
    allow_write: bool,
    retention: Option<clio_config::retention::ResolvedRetention>,
) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
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
    .with_force(opt_bool(args, "allow_duplicate", false));
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
    let dry_run = opt_bool(args, "dry_run", false) || !allow_write;
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

#[cfg(test)]
#[path = "store_write_tests.rs"]
mod tests;
