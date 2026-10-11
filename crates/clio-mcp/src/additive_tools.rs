// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the additive harness workspace tools bound
//! over MCP: the ephemeral scratchpad trio, single-slot canonical
//! tools, cross-agent shared surface, reviewer `validate`, and read-only
//! `stats`.
//!
//! ## Owns
//! - JSON argument decoding and result shaping for these tools
//! - Shared-bank argument forcing (reject a caller `bank`, require `actor`);
//!   the bank-level permission gate itself lives in `McpState::resolve_ctx`
//!
//! ## Does not own
//! - The `batch` engine (see `batch_tools`)
//! - Service semantics (see `clio-compliance` scratchpad/canonical/validate/stats)
//! - Admission gating (runs inside the in-process store path)
//!
//! ## Boundary
//!
//! Changes to additive argument decoding or shared-bank argument forcing should
//! normally require changes here. Changes to scratchpad quotas or canonical
//! supersession should not require changes here.
//!
//! Keep this module focused on additive tool dispatch.

use crate::runtime::{McpState, SHARED_BANK, StderrSinks, StoreSignals};
use crate::write_tools::{gated_write_policy, opt_bool, opt_context, opt_num, opt_str, req_str};
use clio_compliance::{ValidateAction, canonical_get, canonical_subject};
use clio_types::{AmError, ErrorCode, ItemKind, MemoryItem};
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Instant;

/// Route a mutating additive tool (or fall through to the mutator surface).
#[inline(never)]
pub(crate) fn dispatch_inner(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    match name {
        "scratchpad_write" => scratchpad_write(state, args),
        "scratchpad_read" => scratchpad_read(state, args),
        "scratchpad_clear" => scratchpad_clear(state, args),
        "canonical_put" => canonical_put(state, args),
        "shared_store" => shared_store(state, args),
        "shared_discard" => shared_discard(state, args),
        "validate" => validate_tool(state, args),
        "retention_profile_set" => crate::retention_tools::profile_set(state, args),
        "hygiene_clean" => crate::hygiene_tools::dispatch_inner(state, "hygiene_clean", args),
        "sync_push" | "sync_pull" | "sync_serve" | "sync_ack_skip" => {
            crate::sync_tools::dispatch_inner(state, name, args)
        }
        "export" | "import" | "import_provider" => {
            crate::portability_tools::dispatch_inner(state, name, args)
        }
        other => crate::mutator_tools::dispatch_inner(state, other, args),
    }
}

/// Route a read-side additive tool; `None` when the name is not additive.
pub(crate) fn dispatch_read(
    state: &McpState,
    name: &str,
    args: &Value,
) -> Option<Result<Value, AmError>> {
    let result = match name {
        "canonical_get" => canonical_get_tool(state, args),
        "shared_retrieve" => shared_retrieve(state, args),
        "stats" => stats_tool(state, args),
        "hygiene_audit" | "hygiene_log_list" => {
            crate::hygiene_tools::dispatch_inner(state, name, args)
        }
        "sync_status" | "sync_key_status" => crate::sync_tools::dispatch_inner(state, name, args),
        _ => return None,
    };
    Some(result)
}

fn session_of(args: &Value) -> &str {
    opt_str(args, "session").unwrap_or("default")
}

/// `scratchpad_write(key, content)`: ephemeral, quota-bounded write.
fn scratchpad_write(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let key = req_str(args, "key")?;
    let content = req_str(args, "content")?;
    state.scratchpad.write(
        session_of(args),
        ctx.bank.as_str(),
        key,
        content,
        Instant::now(),
    )?;
    Ok(serde_json::json!({
        "stored": true,
        "key": key,
        "bytes": content.len(),
        "ephemeral": true,
        "lifetime": "process"
    }))
}

/// `scratchpad_read(key?)`: read one key or the whole session scratchpad.
fn scratchpad_read(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    state.scratchpad.read(
        session_of(args),
        ctx.bank.as_str(),
        opt_str(args, "key"),
        Instant::now(),
    )
}

/// `scratchpad_clear(key?)`: drop one key or purge the session scratchpad.
fn scratchpad_clear(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    state.scratchpad.clear(
        session_of(args),
        ctx.bank.as_str(),
        opt_str(args, "key"),
        Instant::now(),
    )?;
    Ok(serde_json::json!({ "cleared": true }))
}

/// `canonical_put(key, value, category, epistemic_kind, ...)`: gated single-slot
/// upsert with bi-temporal supersession (prior closed on both time axes).
#[allow(clippy::too_many_lines)]
fn canonical_put(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let key = req_str(args, "key")?;
    let value = req_str(args, "value")?;
    let category = clio_types::SemanticCategory::parse(req_str(args, "category")?)?;
    let epistemic_kind = clio_types::EpistemicKind::parse(req_str(args, "epistemic_kind")?)?;
    let subject = canonical_subject(key);
    let candidate = clio_admission::decision::Candidate::Semantic(category);
    let item_id = args
        .get("item")
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map_or_else(|| state.next_item_id(), str::to_owned);
    let item = MemoryItem {
        id: item_id,
        bank_id: ctx.bank.as_str().to_owned(),
        subject_id: subject,
        kind: ItemKind::Semantic,
        category: Some(category),
        episodic_type: None,
        epistemic_kind,
        source_type: opt_str(args, "source_type")
            .map(clio_types::SourceType::parse)
            .transpose()?,
        confidence: opt_num(args, "confidence"),
        admission_score: None,
        source_ref: None,
        snapshot: args.get("snapshot").cloned().filter(|v| !v.is_null()),
        gist: Some(value.to_owned()),
        valid_from: ts.clone(),
        valid_until: None,
        tx_from: ts.clone(),
        tx_until: None,
        created_at: ts.clone(),
        updated_at: ts.clone(),
        context: opt_context(args, "context")?,
    };
    let dry_run = opt_bool(args, "dry_run", false)?;
    let fields = clio_write::default_event_fields();
    let mut req = clio_write::VerifiedStoreRequest {
        item,
        ctx: &ctx,
        source_text: opt_str(args, "source_text"),
        evidence_ref: opt_str(args, "evidence_ref"),
        fields: &fields,
        verify: clio_write::VerifyConfig::default(),
        policy: gated_write_policy(state, ctx.bank.as_str(), args)?,
        admit_sink: &StderrSinks,
        verify_sink: &StderrSinks,
        timestamp: &ts,
    };
    let mut stored_id: Option<String> = None;
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let decision = clio_write::gated_store_verified(
        &mut req,
        candidate,
        &signals,
        &mut |written: &MemoryItem| {
            if dry_run {
                return Ok(());
            }
            // One transaction resolves the prior slot row and commits the
            // close+create+audit sequence, so a failed step never empties the
            // slot.
            state
                .batch
                .apply_batch(&[clio_store::BatchWrite::CanonicalPut {
                    item: written.clone(),
                    key: key.to_owned(),
                    actor: ctx.actor.as_str().to_owned(),
                }])?;
            let signal = crate::write_tools::attach_leaf(state, written)?;
            state.dispatch_maintenance(vec![signal]);
            stored_id = Some(written.id.clone());
            Ok(())
        },
    )?;
    let mut out = Map::new();
    if let Value::Object(fields) =
        serde_json::to_value(&decision).expect("serialize admission decision")
    {
        out.extend(fields);
    }
    if dry_run {
        out.insert("dry_run".into(), Value::from(true));
    }
    if let Some(id) = stored_id {
        out.insert("id".into(), Value::from(id));
    }
    Ok(Value::Object(out))
}

/// `canonical_get(key, as_of?)`: active or point-in-time slot value.
fn canonical_get_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    match canonical_get(
        &*state.store,
        ctx.bank.as_str(),
        req_str(args, "key")?,
        opt_str(args, "as_of"),
    )? {
        Some(value) => Ok(serde_json::to_value(value).expect("serialize canonical value")),
        None => Err(AmError::new(
            ErrorCode::ItemNotFound,
            format!(
                "canonical slot `{}` is empty",
                args["key"].as_str().unwrap_or("")
            ),
        )),
    }
}

/// Shared-surface authorization gate + forced `bank="shared"` routing.
fn shared_args(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    if !state.shared_enabled {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            format!("`{name}` requires the deployment to enable the shared bank surface"),
        ));
    }
    if opt_str(args, "bank").is_some() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("`{name}` routes exclusively to the `{SHARED_BANK}` bank; do not pass `bank`"),
        ));
    }
    if opt_str(args, "actor").is_none() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("`{name}` requires explicit caller attribution (`actor`)"),
        ));
    }
    let mut shared = args.clone();
    shared["bank"] = Value::from(SHARED_BANK);
    Ok(shared)
}

/// `shared_store(...)`: gated write into the dedicated shared bank.
fn shared_store(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let shared = shared_args(state, "shared_store", args)?;
    Ok(crate::write_tools::dispatch(state, "store", &shared))
}

/// `shared_retrieve(query, ...)`: hybrid search scoped to the shared bank.
fn shared_retrieve(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let shared = shared_args(state, "shared_retrieve", args)?;
    crate::read_retrieve::dispatch(state, "retrieve", &shared)
        .expect("retrieve is a bound read tool")
}

/// `shared_discard(item_id, reason, confirm)`: confirmed ops removal from the
/// shared bank (telemetry written by the discard path).
fn shared_discard(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let shared = shared_args(state, "shared_discard", args)?;
    Ok(crate::write_tools::dispatch(state, "discard", &shared))
}

/// `validate(item_id, action, note?, new_content?)`: reviewer loop routing.
fn validate_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let action = ValidateAction::parse(req_str(args, "action")?)?;
    let ts = state.resolve_timestamp(args);
    let v_args = clio_compliance::ValidateArgs {
        store: &*state.store,
        continuous: &*state.continuous,
        bank_id: ctx.bank.as_str(),
        actor: ctx.actor.as_str(),
        principal: ctx.principal(),
        item_id: req_str(args, "item_id")?,
        action,
        note: opt_str(args, "note"),
        new_content: opt_str(args, "new_content"),
        timestamp: &ts,
        new_item_id: &state.next_item_id(),
    };
    let outcome = clio_compliance::validate(&v_args)?;
    Ok(serde_json::to_value(outcome).expect("serialize validate outcome"))
}

/// Flag band floor: a scanned item at or above this score sits in the hygiene
/// `flag` band. Mirrors the action bands in `clio-hygiene`'s scoring
/// (`suggested_action`, scoring.rs), which publishes no constant, exactly as
/// the pressure block mirrors the same table for its archive and discard
/// bands.
const FLAG_BAND_MIN_SCORE: f64 = 0.40;

/// `stats(bank?, timestamp?)`: read-only counts by bank, tier, category, age
/// band, and provenance class.
///
/// The inputs are what this dispatch already has: the half-life from the
/// ranking environment (`retention.half_life_days`), the one timestamp this
/// dispatch resolves, and the pressure counts re-read from the pressure
/// block's own sources. A pressure source that fails or was cut short by its
/// scan budget leaves its count out of the report instead of fabricating a
/// zero, and never fails the whole tool.
fn stats_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let now = state.resolve_timestamp(args);
    let pressure = stats_pressure(state, ctx.bank.as_str(), &now);
    let report = clio_compliance::stats(
        &*state.store,
        ctx.bank.as_str(),
        &clio_compliance::StatsInputs {
            band_unit_days: state.env.retention.half_life_days,
            now,
            pressure: Some(pressure),
        },
    )?;
    Ok(serde_json::to_value(report).expect("serialize stats report"))
}

/// The three pressure counts the report relays, one per source the pressure
/// block reads: `flagged` from the bounded hygiene flag-band scan below,
/// `stale summaries` from the stored distilled-unit records
/// (`pressure::stale_units`), and `archive candidates` from the bounded
/// hygiene archive-band scan (`pressure::archive_candidates`). Each source
/// answers with its count or nothing, never a fabricated zero.
fn stats_pressure(state: &McpState, bank: &str, now: &str) -> clio_compliance::PressureCounts {
    clio_compliance::PressureCounts {
        flagged: flag_band_count(state, bank, now),
        stale_summaries: pressure_count(&crate::pressure::stale_units(state, bank)),
        archive_candidates: pressure_count(&crate::pressure::archive_candidates(
            state,
            bank,
            now,
            crate::pressure::ARCHIVE_SCAN_BUDGET,
        )),
    }
}

/// The `count` of a pressure entry, or `None` when the entry is the
/// unavailable marker or its scan was cut short by the budget (`incomplete`):
/// a count the source cannot stand behind is omitted, never relayed partial.
fn pressure_count(entry: &Value) -> Option<u64> {
    if entry.get("incomplete") == Some(&Value::from(true)) {
        return None;
    }
    entry.get("count").and_then(Value::as_u64)
}

/// Flagged count: the hygiene `flag` band ([`FLAG_BAND_MIN_SCORE`] to below
/// the pressure block's archive floor), counted as the difference between two
/// bounded audits over the same row prefix, mirroring the pressure block's
/// two-scan band pattern (`archive_candidates`): the scan at the flag floor
/// covers flag-, archive-, and discard-band items, the scan at the archive
/// floor covers archive and discard, so the difference is exactly the flag
/// band and a discard-band item (such as a secret hit) is never counted. Both
/// scans run under the pressure block's published scan budget; a budget-cut
/// scan yields `None`.
fn flag_band_count(state: &McpState, bank: &str, now: &str) -> Option<u64> {
    let args = |min_score: f64| clio_hygiene::AuditArgs {
        store: &*state.store,
        bank,
        now,
        min_score,
        limit: crate::pressure::ARCHIVE_PAGE_SIZE,
        offset: 0,
        weights: &state.env.hygiene,
        stale_half_life_days: state.env.hygiene_stale_half_life_days,
        max_scan: crate::pressure::ARCHIVE_SCAN_BUDGET,
    };
    let through_flag = clio_hygiene::hygiene_audit(&args(FLAG_BAND_MIN_SCORE)).ok()?;
    let through_archive =
        clio_hygiene::hygiene_audit(&args(crate::pressure::ARCHIVE_BAND_MIN_SCORE)).ok()?;
    if through_flag.incomplete {
        return None;
    }
    Some(
        through_flag
            .total_candidates
            .saturating_sub(through_archive.total_candidates) as u64,
    )
}

#[cfg(test)]
#[path = "additive_tools_shared_tests.rs"]
mod additive_tools_shared_tests;
#[cfg(test)]
#[path = "additive_tools_test_support_tests.rs"]
mod additive_tools_test_support_tests;
#[cfg(test)]
#[path = "additive_tools_tests.rs"]
mod additive_tools_tests;
