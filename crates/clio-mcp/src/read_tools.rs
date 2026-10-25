// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP read-surface dispatcher: routing the
//! bound read tools to their in-process handlers and shaping results.
//!
//! ## Owns
//! - Read-tool routing and shared argument helpers (`limit`, domain lists,
//!   `time_axis`)
//! - Item primitives (`get`, `get_snapshot`, `get_gist`, `maintenance_status`),
//!   `persona_get`, and the `inspect` listing
//!
//! ## Does not own
//! - Retrieval/compose/graph handlers (see `read_retrieve`)
//! - History/triple/belief handlers (see `read_history`)
//! - Transport framing (see `protocol`/`stdio`/`http`)
//!
//! ## Boundary
//!
//! Changes to read routing or item-level shaping should normally require
//! changes here. Changes to a specific tool family belong in its submodule.
//!
//! Keep this module focused on read dispatch plus item-level reads.

use crate::runtime::McpState;
use crate::write_tools::{opt_str, req_str, strict_string, validate_time_bound};
use chrono::{DateTime, SecondsFormat, Utc};
use clio_types::{AmError, ErrorCode, TimeAxis, ToolResult};
use serde::Serialize;
use serde_json::{Value, json};

#[path = "read_freshness.rs"]
mod read_freshness;

#[path = "read_inspect_supersession.rs"]
mod read_inspect_supersession;

/// Route one bound read tool to its in-process handler.
pub(crate) fn dispatch(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    // Config reads and caller-supplied previews do not imply a stored bank.
    if !matches!(
        name,
        "config_get"
            | "config_profiles"
            | "ranking_env_get"
            | "admit_preview_batch"
            | "retention_profile_get"
    ) {
        let ctx = state.resolve_ctx(args)?;
        state.require_known_bank(ctx.bank.as_str())?;
    }
    match name {
        "get" => get_item(state, args),
        "get_snapshot" => get_snapshot(state, args),
        "get_gist" => get_gist(state, args),
        "maintenance_status" => maintenance_status(state, args),
        "persona_get" => persona_get(state, args),
        "inspect" => inspect(state, args),
        "audit_trail" => audit_trail(state, args),
        "summarize" => crate::summarize_tools::summarize(state, args),
        "config_get" | "config_profiles" | "ranking_env_get" => {
            crate::config_tools::dispatch_read(state, name, args)
        }
        "admit_preview_batch" => crate::retention_tools::preview_batch(state, args),
        "retention_profile_get" => crate::retention_tools::profile_get(state, args),
        other => {
            if let Some(result) = crate::additive_tools::dispatch_read(state, other, args) {
                result
            } else if let Some(result) = crate::read_retrieve::dispatch(state, other, args) {
                result
            } else if let Some(result) = crate::read_history::dispatch(state, other, args) {
                result
            } else {
                Err(AmError::not_implemented(other))
            }
        }
    }
}

/// Optional unsigned-integer argument (`limit`, `offset`, `depth`, `max_hops`).
pub(crate) fn opt_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
}

/// Optional `u32` argument (`depth`).
pub(crate) fn opt_u32(args: &Value, key: &str) -> Option<u32> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
}

/// String-array argument; non-string entries are skipped (never fails).
pub(crate) fn str_list(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the optional `time_axis` argument (default `valid`).
pub(crate) fn time_axis(args: &Value) -> Result<TimeAxis, AmError> {
    match opt_str(args, "time_axis") {
        None => Ok(TimeAxis::Valid),
        Some(raw) => TimeAxis::parse(raw),
    }
}

/// Shape an in-process `ToolResult` into a JSON success or error.
pub(crate) fn tool_result_value<T: Serialize>(result: ToolResult<T>) -> Result<Value, AmError> {
    match result {
        ToolResult::Ok { data, .. } => {
            Ok(serde_json::to_value(data).expect("serialize tool payload"))
        }
        ToolResult::Err { code, message, .. } => Err(AmError::new(code, message)),
    }
}

pub(crate) fn item_not_found(item_id: &str) -> AmError {
    AmError::new(
        ErrorCode::ItemNotFound,
        format!("item `{item_id}` not found in bank"),
    )
}

/// Refuse an item id the bank does not hold. A mistyped id is then an error,
/// not an empty answer that reads as "no data".
pub(crate) fn require_item(state: &McpState, bank: &str, item_id: &str) -> Result<(), AmError> {
    match state.store.get_memory_item(bank, item_id)? {
        Some(_) => Ok(()),
        None => Err(item_not_found(item_id)),
    }
}

/// `get(item_id)`: full metadata + content references.
///
/// Two additive keys carry the stored row's lifecycle state, so a soft-deleted
/// item still reads by id with its delete marker: `discarded` (operations
/// removal) and `archived` (hygiene marker). A distilled unit carries one more
/// key, `freshness`: its proof count, last reconciliation, stale state and
/// reason, and its per-revision evidence trail (ids and times only).
#[inline(never)]
fn get_item(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let item_id = req_str(args, "item_id")?;
    let Some(item) = state.store.get_memory_item(ctx.bank.as_str(), item_id)? else {
        return Err(item_not_found(item_id));
    };
    let block = read_freshness::block_for(
        &*state.store,
        ctx.bank.as_str(),
        item_id,
        item.snapshot.as_ref(),
    )?;
    // The identity read excludes a discarded row, so `None` here (with the
    // item present) is the discard marker.
    let discarded = state
        .store
        .get_item_identity(ctx.bank.as_str(), item_id)?
        .is_none();
    let archived = state.store.item_is_archived(ctx.bank.as_str(), item_id)?;
    let mut value = serde_json::to_value(&item).expect("serialize memory item");
    if let Some(object) = value.as_object_mut() {
        object.insert("discarded".into(), Value::from(discarded));
        object.insert("archived".into(), Value::from(archived));
        if let Some(block) = block {
            object.insert(read_freshness::FRESHNESS_KEY.into(), block);
        }
    }
    Ok(value)
}

/// `get_snapshot(item_id)`: authoritative structured half only.
#[inline(never)]
fn get_snapshot(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let item_id = req_str(args, "item_id")?;
    match state.store.get_snapshot(ctx.bank.as_str(), item_id)? {
        Some(snapshot) => Ok(json!({
            "item_id": item_id,
            "authoritative": true,
            "snapshot": snapshot
        })),
        None => Err(item_not_found(item_id)),
    }
}

/// `get_gist(item_id)`: non-authoritative prose half only.
#[inline(never)]
fn get_gist(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let item_id = req_str(args, "item_id")?;
    match state.store.get_gist(ctx.bank.as_str(), item_id)? {
        Some(gist) => Ok(json!({
            "item_id": item_id,
            "authoritative": false,
            "gist": gist
        })),
        None => Err(item_not_found(item_id)),
    }
}

/// `maintenance_status(item_id?)`: leaf queryability, dirty ancestors, the
/// live index/rerank/extraction pipeline state, the read-only pressure
/// block (counts of waiting maintenance work, one entry per component), and
/// the trigger block (the automatic wave trigger's last recorded decision).
#[inline(never)]
fn maintenance_status(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    if let Some(item_id) = opt_str(args, "item_id") {
        require_item(state, ctx.bank.as_str(), item_id)?;
    }
    let result =
        clio_write::maintenance_status(&state.maint, ctx.bank.as_str(), opt_str(args, "item_id"));
    let status = match result.clone() {
        ToolResult::Ok { data, .. } => data,
        // `tool_result_value` below sees the same failure and returns it.
        ToolResult::Err { .. } => return tool_result_value(result),
    };
    let mut value = tool_result_value(result)?;
    // One drain-status read feeds both the pipeline block and the pressure
    // block, so one response can never carry two different pending counts,
    // and a failed read renders its own unavailable marker instead of failing
    // the whole call.
    let drain = state.index_drain.status(Some(ctx.bank.as_str()));
    value["index"] = state.index_status_value(&drain);
    let now = state.resolve_timestamp(args)?;
    value["pressure"] = crate::pressure::block(state, ctx.bank.as_str(), &status, &now, &drain);
    // The trigger block reads the trigger's own recorded decision, next to
    // the pressure block it decides from.
    value["trigger"] = crate::pressure::trigger_block(
        state.maintenance_trigger.status(ctx.bank.as_str()),
        std::time::Instant::now(),
    );
    Ok(value)
}

/// `persona_get()`: bounded persona document under the persona budget.
#[inline(never)]
fn persona_get(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let timestamp = state.resolve_timestamp(args)?;
    let budget =
        opt_usize(args, "budget_tokens").unwrap_or(clio_persona::DEFAULT_PERSONA_TOKEN_BUDGET);
    let outcome = clio_persona::persona_get(&clio_persona::PersonaGetArgs {
        store: &*state.persona,
        ctx: &ctx,
        budget_tokens: budget,
        timestamp: &timestamp,
    })?;
    Ok(json!({
        "document": outcome.document,
        "dropped_keys": outcome.dropped_keys
    }))
}

/// `inspect(filter?, limit?, offset?)`: item listing with optional context.
///
/// Returns metadata rows plus the optional decrypted `context` for readable
/// items — never ciphertext, DEKs, or payloads. A consolidated row additionally
/// carries `freshness` for the unit it is, read on the same item read that
/// attaches `context`. A defensive secret-mask pass runs over the payload before
/// it leaves. The listing never aborts because one row's sealed body is
/// unreadable (missing DEK, DEK version mismatch, AEAD authentication failure,
/// crypto-shredded subject, or legacy opaque row): that row keeps its metadata
/// and simply omits `context`. The only caller authorization here is the
/// bank-scoped query itself; a direct `get` on the id still reports the
/// underlying per-row error.
#[inline(never)]
fn inspect(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let scope = match args.get("filter") {
        None | Some(Value::Null) => None,
        Some(Value::Object(_)) => args.get("filter"),
        Some(_) => {
            return Err(AmError::new(
                ErrorCode::InvalidType,
                "filter must be an object",
            ));
        }
    };
    if let Some(fields) = scope.and_then(Value::as_object)
        && let Some(key) = fields
            .keys()
            .find(|key| !INSPECT_FILTER_KEYS.contains(&key.as_str()))
    {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("unknown filter key `{key}`"),
        ));
    }
    let category = inspect_filter_string(scope, "category")?;
    if let Some(value) = category {
        clio_types::SemanticCategory::parse(value)?;
    }
    let epistemic_kind = inspect_filter_string(scope, "epistemic_kind")?;
    if let Some(value) = epistemic_kind {
        clio_types::EpistemicKind::parse(value)?;
    }
    let time_start = inspect_filter_string(scope, "time_start")?
        .map(|value| normalize_time_bound("filter.time_start", value))
        .transpose()?;
    let time_end = inspect_filter_string(scope, "time_end")?
        .map(|value| normalize_time_bound("filter.time_end", value))
        .transpose()?;
    let mut filter = clio_store::ItemInspectFilter::new(ctx.bank.as_str());
    filter.category = category.map(str::to_owned);
    filter.epistemic_kind = epistemic_kind.map(str::to_owned);
    if let Some(raw) = inspect_filter_string(scope, "validity")? {
        filter.validity = clio_store::InspectValidity::parse(raw)?;
    }
    filter.time_start = time_start;
    filter.time_end = time_end;
    filter.include_discarded = match scope.and_then(|value| value.get("include_discarded")) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => {
            return Err(AmError::new(
                ErrorCode::InvalidType,
                "filter.include_discarded must be a boolean",
            ));
        }
    };
    filter.limit = opt_usize(args, "limit").unwrap_or(50).min(200);
    filter.offset = opt_usize(args, "offset").unwrap_or(0);
    let rows = state.store.inspect_items(&filter)?;
    let total = state.store.count_items(&filter)?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let mut value = serde_json::to_value(&row).expect("serialize inspect row");
        // No stored row, or an unreadable sealed body — missing DEK, DEK
        // version mismatch, AEAD authentication failure, crypto-shredded
        // subject, or a legacy-format row restored through the opaque
        // ciphertext_backup route — leaves the metadata
        // row intact and simply omits `context`. Same resilience policy as
        // `clio_store`'s `open_context` used by audit views and as
        // retrieval's unreadable-item skip.
        if let Ok(Some(item)) = state.store.get_memory_item(ctx.bank.as_str(), &row.item_id) {
            if let Some(context) = item.context
                && let Some(object) = value.as_object_mut()
            {
                object.insert("context".into(), Value::from(context));
            }
            // The same item read resolves the unit this row is, so a
            // consolidated row reports its freshness without a second item
            // read. Unlike an unreadable body, a failed freshness read is a
            // storage failure rather than a per-row content condition, so it
            // fails the call instead of silently dropping the block.
            if let Some(block) = read_freshness::block_for(
                &*state.store,
                ctx.bank.as_str(),
                &row.item_id,
                item.snapshot.as_ref(),
            )? && let Some(object) = value.as_object_mut()
            {
                object.insert(read_freshness::FRESHNESS_KEY.into(), block);
            }
        }
        items.push(value);
    }
    let mut payload = json!({
        "items": items,
        "total": total,
        "limit": filter.limit,
        "offset": filter.offset,
        "include_discarded": filter.include_discarded
    });
    // Optional read-only supersession section for one item; absent unless
    // the caller names an `item_id`.
    if let Some(item_id) = opt_str(args, "item_id") {
        payload["supersession"] = read_inspect_supersession::section(state, &ctx, args, item_id)?;
    }
    // Defence in depth: mask any secret-looking field that ever appears here.
    clio_config::secret::mask_value("", &mut payload);
    Ok(payload)
}

const INSPECT_FILTER_KEYS: &[&str] = &[
    "category",
    "epistemic_kind",
    "validity",
    "time_start",
    "time_end",
    "include_discarded",
];

/// Read an optional non-empty string selector from an inspect filter.
fn inspect_filter_string<'a>(
    filter: Option<&'a Value>,
    key: &str,
) -> Result<Option<&'a str>, AmError> {
    match filter.and_then(|value| value.get(key)) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => strict_string(value, &format!("filter.{key}")).map(Some),
    }
}

fn normalize_time_bound(key: &str, value: &str) -> Result<String, AmError> {
    validate_time_bound(key, value)?;
    Ok(DateTime::parse_from_rfc3339(value).map_or_else(
        |_| value.to_owned(),
        |timestamp| {
            timestamp
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::AutoSi, true)
        },
    ))
}

/// `audit_trail(item_id)`: masked lifecycle reconstruction.
///
/// Traverses `corrects_item_id`/`superseded_by_id` lineage both ways and
/// stitches telemetry rows, revisions, and belief confidence history; a
/// secret-mask pass runs over the payload before it leaves.
#[inline(never)]
fn audit_trail(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let item_id = req_str(args, "item_id")?;
    let view = clio_compliance::audit_trail(&*state.store, ctx.bank.as_str(), item_id)?;
    Ok(serde_json::to_value(view).expect("serialize audit trail"))
}

#[cfg(test)]
#[path = "read_tools_tests.rs"]
mod read_tools_tests;

#[cfg(test)]
#[path = "read_tools_filter_validation_tests.rs"]
mod read_tools_filter_validation_tests;

#[cfg(test)]
#[path = "read_tools_bank_tests.rs"]
mod read_tools_bank_tests;

#[cfg(test)]
#[path = "read_tools_inspect_time_tests.rs"]
mod read_tools_inspect_time_tests;
