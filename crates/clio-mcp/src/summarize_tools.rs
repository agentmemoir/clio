// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP `summarize` tool: regenerating the
//! non-authoritative prose gist for a caller-scoped set of items from their
//! existing snapshots, without ever altering a snapshot.
//!
//! ## Owns
//! - Scope resolution (`scope` = `bank` or one item id in the caller's bank)
//! - Feeding each item's snapshot through the configured extractor's existing
//!   gist generation and persisting only the regenerated gist
//! - The no-write `dry_run` report and the structured report (summarized /
//!   skipped with reasons)
//!
//! ## Does not own
//! - Text production (the `clio_write::Extractor` backends own that; this
//!   module never fabricates gist text itself)
//! - Snapshot content or storage (`clio-store`; snapshots are read-only here)
//! - Extraction provider selection rules (`clio_write::build_extractor_from`)
//!
//! ## Boundary
//!
//! Changes to summarize scoping, reporting, or the snapshot→gist hand-off
//! should normally require changes here. Changes to extraction I/O or provider
//! rules should not require changes here.
//!
//! Keep this module focused on the MCP summarize surface.

use crate::runtime::McpState;
use crate::write_tools::{opt_bool, req_str};
use clio_config::bearer_for_effective;
use clio_store::{InspectValidity, ItemInspectFilter};
use clio_types::{AmError, ErrorCode, MemoryItem, OpContext};
use clio_write::{ExtractRequest, Extractor, HttpTransport, Transport, build_extractor_from};
use serde_json::{Value, json};
use std::sync::Arc;

/// Bank-scope page size for the deterministic `inspect_items` walk.
const PAGE: usize = 200;

/// `summarize(scope)`: regenerate gist(s) for a scope; snapshots never change.
///
/// # Errors
/// Unknown scope item, extraction unavailable, or a store failure while
/// persisting a regenerated gist.
pub(crate) fn summarize(state: &McpState, args: &Value) -> Result<Value, AmError> {
    summarize_with_transport(state, args, HttpTransport::default())
}

/// Summarize through an explicit transport, resolving the extractor from the
/// effective config. The transport seam lets tests exercise the configured
/// provider wiring (factory + adapter) without network I/O.
///
/// # Errors
/// As [`summarize`].
pub(crate) fn summarize_with_transport<T: Transport + 'static>(
    state: &McpState,
    args: &Value,
    transport: T,
) -> Result<Value, AmError> {
    let extractor = configured_extractor(state, transport)?;
    summarize_with_extractor(state, args, &*extractor)
}

/// Build the extractor resolved from the effective config, failing closed
/// when no gist generator is configured (same rule as `clio_write::ingest_raw`).
///
/// # Errors
/// Extraction unavailable (no configured endpoint or hosted credential).
fn configured_extractor<T: Transport + 'static>(
    state: &McpState,
    transport: T,
) -> Result<Arc<dyn Extractor>, AmError> {
    let effective = {
        let rt = state.config.lock().expect("config runtime mutex poisoned");
        rt.effective_raw()
    };
    if !crate::runtime::extract_available_from(&effective) {
        return Err(AmError::new(
            ErrorCode::NotImplemented,
            "extraction unavailable: no extractor configured (set extract.url to a local \
             extraction sidecar, or extract.provider=openai with a credential); no gist was \
             regenerated and no text was fabricated",
        ));
    }
    let bearer = bearer_for_effective(&effective, "extract");
    build_extractor_from(&effective, bearer, transport)
}

/// Summarize with an explicit extractor (injectable seam, mirroring
/// `clio_write::ingest_raw_with_extractor`).
///
/// # Errors
/// Unknown scope item, or a store failure while persisting a regenerated gist.
pub(crate) fn summarize_with_extractor(
    state: &McpState,
    args: &Value,
    extractor: &dyn Extractor,
) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let scope = req_str(args, "scope")?;
    let dry_run = opt_bool(args, "dry_run", false)?;
    if scope == "bank" {
        summarize_bank(state, &ctx, dry_run, extractor)
    } else {
        summarize_item(state, &ctx, scope, dry_run, extractor)
    }
}

fn summarize_item(
    state: &McpState,
    ctx: &OpContext,
    item_id: &str,
    dry_run: bool,
    extractor: &dyn Extractor,
) -> Result<Value, AmError> {
    if state.store.item_is_archived(ctx.bank.as_str(), item_id)? {
        return Ok(skipped_only(item_id, ctx, item_id, "archived", dry_run));
    }
    let Some(item) = state.store.get_memory_item(ctx.bank.as_str(), item_id)? else {
        return Err(AmError::new(
            ErrorCode::ItemNotFound,
            format!("item `{item_id}` not found in bank"),
        ));
    };
    // `get_item_identity` excludes operationally discarded rows, so `None` here
    // (with the item present) means discarded: never summarize an inactive item.
    if state
        .store
        .get_item_identity(ctx.bank.as_str(), item_id)?
        .is_none()
    {
        return Ok(skipped_only(item_id, ctx, item_id, "discarded", dry_run));
    }
    let mut summarized = Vec::new();
    let mut skipped = Vec::new();
    regenerate(
        state,
        ctx,
        &item,
        dry_run,
        extractor,
        &mut summarized,
        &mut skipped,
    )?;
    Ok(report(item_id, ctx, dry_run, &summarized, &skipped))
}

fn summarize_bank(
    state: &McpState,
    ctx: &OpContext,
    dry_run: bool,
    extractor: &dyn Extractor,
) -> Result<Value, AmError> {
    let mut summarized = Vec::new();
    let mut skipped = Vec::new();
    let mut offset = 0_usize;
    loop {
        let rows = state
            .store
            .inspect_items(&bank_filter(ctx.bank.as_str(), offset))?;
        let full_page = rows.len() == PAGE;
        for row in &rows {
            // Archived and discarded items are excluded from active use; report
            // them as skipped so the bank report is complete (never silent).
            if row.archived {
                skipped.push(skip_entry(&row.item_id, "archived"));
                continue;
            }
            if row.discarded {
                skipped.push(skip_entry(&row.item_id, "discarded"));
                continue;
            }
            if let Some(item) = state
                .store
                .get_memory_item(ctx.bank.as_str(), &row.item_id)?
            {
                regenerate(
                    state,
                    ctx,
                    &item,
                    dry_run,
                    extractor,
                    &mut summarized,
                    &mut skipped,
                )?;
            }
        }
        if !full_page {
            break;
        }
        offset += PAGE;
    }
    Ok(report("bank", ctx, dry_run, &summarized, &skipped))
}

fn bank_filter(bank: &str, offset: usize) -> ItemInspectFilter {
    let mut filter = ItemInspectFilter::new(bank);
    filter.validity = InspectValidity::All;
    filter.include_discarded = true;
    filter.limit = PAGE;
    filter.offset = offset;
    filter
}

/// Regenerate one item's gist from its existing snapshot and persist only the
/// gist half. Snapshots are never written here; `dry_run` skips the persist.
fn regenerate(
    state: &McpState,
    ctx: &OpContext,
    item: &MemoryItem,
    dry_run: bool,
    extractor: &dyn Extractor,
    summarized: &mut Vec<Value>,
    skipped: &mut Vec<Value>,
) -> Result<(), AmError> {
    let Some(snapshot) = item.snapshot.clone() else {
        skipped.push(skip_entry(&item.id, "no snapshot to summarize from"));
        return Ok(());
    };
    let request = ExtractRequest {
        context: None,
        source_text: serde_json::to_string(&snapshot).expect("serialize snapshot as source text"),
        tool_payload: None,
        schema_template: snapshot.clone(),
        verify_feedback: None,
    };
    let gist = match extractor.extract(&request) {
        Ok(candidate) => candidate.gist.filter(|g| !g.trim().is_empty()),
        Err(e) => {
            skipped.push(json!({
                "item_id": item.id,
                "reason": format!("extraction failed: {e}"),
            }));
            return Ok(());
        }
    };
    let Some(gist) = gist else {
        skipped.push(skip_entry(&item.id, "extractor produced no gist"));
        return Ok(());
    };
    if !dry_run {
        let mut updated = item.clone();
        updated.gist = Some(gist.clone());
        updated.updated_at = crate::runtime::now_utc();
        state
            .store
            .update_memory_item(&updated, ctx.actor.as_str())?;
    }
    summarized.push(json!({
        "item_id": item.id,
        "gist": gist,
    }));
    Ok(())
}

fn skip_entry(item_id: &str, reason: &str) -> Value {
    json!({ "item_id": item_id, "reason": reason })
}

/// A report with exactly one skipped item and no summarized items.
fn skipped_only(scope: &str, ctx: &OpContext, item_id: &str, reason: &str, dry_run: bool) -> Value {
    report(scope, ctx, dry_run, &[], &[skip_entry(item_id, reason)])
}

fn report(
    scope: &str,
    ctx: &OpContext,
    dry_run: bool,
    summarized: &[Value],
    skipped: &[Value],
) -> Value {
    json!({
        "scope": scope,
        "bank": ctx.bank.as_str(),
        "dry_run": dry_run,
        "count": {
            "summarized": summarized.len(),
            "skipped": skipped.len(),
        },
        "summarized": summarized,
        "skipped": skipped,
    })
}

#[cfg(test)]
#[path = "summarize_tools_tests.rs"]
mod summarize_tools_tests;
