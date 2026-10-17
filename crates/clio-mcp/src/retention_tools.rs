// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP retention-profile surface: profile
//! get/set and the read-only batch admission preview.
//!
//! ## Owns
//! - Argument decoding and result shaping for `retention_profile_get`,
//!   `retention_profile_set`, and `admit_preview_batch`
//! - The documented batch-size cap and the per-item reason buckets
//!
//! ## Does not own
//! - Profile storage/resolution (see `runtime_retention` and `clio-config`)
//! - The single-item scoring path reused per batch item (see `write_tools`)
//!
//! ## Boundary
//!
//! Changes to retention tool argument or response shape should normally require
//! changes here. Changes to profile semantics should not require changes here.
//!
//! Keep this module focused on the MCP retention-profile surface.

use crate::runtime::McpState;
use crate::write_tools::{opt_bool, opt_str};
use clio_config::retention::{ProvenanceFloorDefault, RETENTION_PROFILE_VERSION, RetentionProfile};
use clio_types::{AmError, ErrorCode};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Largest batch a single `admit_preview_batch` call will score.
pub const MAX_PREVIEW_BATCH: usize = 50;

/// Machine-readable bucket for a per-item rejection reason.
fn reason_bucket(reason: Option<&str>) -> &'static str {
    let Some(reason) = reason else {
        return "unspecified";
    };
    if reason.starts_with("mission_drop") {
        "mission_drop"
    } else if reason.starts_with("category gate") {
        "category_gate"
    } else if reason.starts_with("episodic_type gate") {
        "episodic_type_gate"
    } else if reason.starts_with("duplicate tolerance") {
        "duplicate_tolerance"
    } else if reason.starts_with("score ") {
        "below_threshold"
    } else {
        "other"
    }
}

fn parse_profile(value: &Value, what: &str) -> Result<RetentionProfile, AmError> {
    let profile: RetentionProfile = serde_json::from_value(value.clone())
        .map_err(|e| AmError::new(ErrorCode::InvalidType, format!("invalid {what}: {e}")))?;
    profile.validate()?;
    Ok(profile)
}

/// `retention_profile_get(bank?)`: the effective profile plus the inert fields.
pub(crate) fn profile_get(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let (profile, source) = state.retention_effective(ctx.bank.as_str());
    let mission = profile.mission.as_ref().map(|m| {
        json!({
            "statement": m.mission_statement,
            "keep_examples": m.keep_examples.len(),
            "drop_examples": m.drop_examples.len(),
            "version": RETENTION_PROFILE_VERSION,
        })
    });
    Ok(json!({
        "bank": ctx.bank.as_str(),
        "source": source.as_str(),
        "verbosity": profile.verbosity.as_str(),
        "theta_offsets": profile.theta_offsets,
        "duplicate_tolerance_write": profile.duplicate_tolerance_write.as_str(),
        "retention_curve": profile.retention_curve,
        "retention_floor": profile.retention_floor,
        "pack_fidelity_default": profile.pack_fidelity_default.as_str(),
        "provenance_floor_default": profile
            .provenance_floor_default
            .map(ProvenanceFloorDefault::as_str),
        "maintenance_trigger": profile.maintenance_trigger,
        "version": profile.version,
        "mission": mission,
        "accepted_but_inert": profile.accepted_but_inert(),
    }))
}

/// `retention_profile_set(bank?, profile, scope?, confirm?)`.
///
/// Without `confirm=true` this is a no-write preview of what would be stored
/// (harness confirmation).
pub(crate) fn profile_set(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let raw = args
        .get("profile")
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "profile is required"))?;
    let profile = parse_profile(raw, "retention profile")?;
    let scope = opt_str(args, "scope").unwrap_or("bank");
    if scope != "bank" && scope != "deployment" {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("scope must be bank|deployment, got `{scope}`"),
        ));
    }
    if !opt_bool(args, "confirm", false)? {
        return Ok(json!({
            "confirm": false,
            "stored": false,
            "bank": ctx.bank.as_str(),
            "scope": scope,
            "would_store": profile,
            "accepted_but_inert": profile.accepted_but_inert(),
        }));
    }
    let persisted = if scope == "deployment" {
        state.authorize_admin()?;
        state.set_retention_deployment(profile.clone())?
    } else {
        state.set_retention(ctx.bank.as_str(), profile.clone())?
    };
    Ok(json!({
        "confirm": true,
        "stored": true,
        "persisted": persisted,
        "bank": ctx.bank.as_str(),
        "scope": scope,
        "profile": profile,
        "accepted_but_inert": profile.accepted_but_inert(),
    }))
}

/// `admit_preview_batch(bank?, items[], profile_override?)`: read-only dry run.
///
/// Runs the single-item scoring path once per item, in input order, writing
/// nothing. `profile_override` previews a candidate profile without persisting it.
pub(crate) fn preview_batch(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let items = args.get("items").and_then(Value::as_array).ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "items must be an array of candidate objects",
        )
    })?;
    if items.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "items must contain at least one candidate",
        ));
    }
    if items.len() > MAX_PREVIEW_BATCH {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "items length {} exceeds the preview limit of {MAX_PREVIEW_BATCH}; split the batch and retry",
                items.len()
            ),
        ));
    }
    let override_profile = match args.get("profile_override").filter(|v| !v.is_null()) {
        Some(raw) => Some(parse_profile(raw, "profile_override")?),
        None => None,
    };
    let (effective, source) = state.retention_effective(ctx.bank.as_str());
    let retention = override_profile.clone().unwrap_or(effective);
    let resolved = retention.resolve();
    let bank = ctx.bank.as_str();
    let actor = ctx.actor.as_str();

    let mut results: Vec<Value> = Vec::with_capacity(items.len());
    let mut would_admit = 0_usize;
    let mut by_reason: BTreeMap<String, usize> = BTreeMap::new();
    for raw in items {
        let mut fields: Map<String, Value> = raw.as_object().cloned().ok_or_else(|| {
            AmError::new(
                ErrorCode::InvalidArgument,
                "each batch item must be an object",
            )
        })?;
        fields.insert("bank".into(), Value::from(bank));
        fields.insert("actor".into(), Value::from(actor));
        let item_args = Value::Object(fields);
        match crate::store_write::store_path(state, &item_args, false, Some(resolved.clone())) {
            Ok(decision) => {
                if decision.get("pass").and_then(Value::as_bool) == Some(true) {
                    would_admit += 1;
                } else {
                    let bucket =
                        reason_bucket(decision.get("rejection_reason").and_then(Value::as_str));
                    *by_reason.entry(bucket.to_owned()).or_insert(0) += 1;
                }
                results.push(decision);
            }
            Err(e) => {
                // Per-item factor/validation failure: report it without scoring
                // or writing anything for that item (no partial writes exist).
                *by_reason.entry("error".to_owned()).or_insert(0) += 1;
                results.push(json!({
                    "pass": false,
                    "error": {"code": e.code, "message": e.message},
                    "rejection_reason": e.message,
                }));
            }
        }
    }
    // A preview where every item failed validation previewed nothing, so it is a
    // refusal, not a success. The CLI's `admit --file` gives the same refusal.
    if let Some(first) = results.first()
        && results.iter().all(|item| item.get("error").is_some())
    {
        let reason = first
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("invalid item");
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "no item previewed: all {} items failed validation; item 0: {reason}",
                results.len()
            ),
        ));
    }
    let count = results.len();
    Ok(json!({
        "bank": bank,
        "profile_source": if override_profile.is_some() { "override" } else { source.as_str() },
        "profile_version": resolved.profile_version(),
        "dry_run": true,
        "read_only": true,
        "writes": 0,
        "count": count,
        "summary": {
            "would_admit": would_admit,
            "would_reject": count - would_admit,
            "by_reason": by_reason,
        },
        "items": results,
    }))
}

#[cfg(test)]
#[path = "retention_tools_tests.rs"]
mod retention_tools_tests;

#[cfg(test)]
#[path = "retention_tools_preview_tests.rs"]
mod preview_tests;
