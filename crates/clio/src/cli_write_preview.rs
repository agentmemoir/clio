// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the no-write preview path of the write
//! engine for `summarize` and `consolidate`: validating a `--dry-run` against
//! the store it would touch (bank resolution, unknown-bank refusal, item
//! scope) and computing the read-only pending-work counts the preview reports.
//!
//! ## Owns
//! - `validate_preview`: the bank/scope check a dry run shares with the real
//!   call's refusal behavior
//! - `preview_counts`: the read-only counts object a dry run reports next to
//!   its `would` sentence
//!
//! ## Does not own
//! - The write engine that emits the preview (see `cli_write`)
//! - The preview payload shapes built per verb (see `cli_write_core`)
//! - The real summarize and consolidate calls (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to what a dry run checks or counts should normally require changes
//! here. Changes to preview emission or the real tool calls should not require
//! changes here.
//!
//! Keep this module focused on the dry-run preview checks and counts.

use crate::cli_args::ParsedArgs;
use crate::cli_error::CliError;
use clio_mcp::runtime::{McpState, extract_available_from};
use clio_store::{InspectValidity, ItemInspectFilter};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};

/// Validate a `--dry-run` preview against the store it would touch: resolve
/// the caller's `--bank` (or the runtime default) and refuse an unknown or
/// invalid bank exactly as the real tool call would, so every dry run of this
/// dispatcher reports a missing bank the same way. `summarize` additionally
/// checks its scope: a bank scope is fine, but an item scope must name an
/// existing item. Returns the resolved bank, which the pending-work counts
/// read against.
///
/// # Errors
/// A usage or store error when the dry run could not touch the store the way
/// the real call would.
pub(crate) fn validate_preview(
    state: &McpState,
    verb: &str,
    parsed: &ParsedArgs,
) -> Result<String, CliError> {
    let mut args = serde_json::Map::new();
    if let Some(bank) = parsed.value("bank") {
        args.insert("bank".into(), serde_json::Value::from(bank));
    }
    let ctx = state
        .resolve_ctx(&serde_json::Value::Object(args))
        .map_err(|error| CliError::from_am_error(&error))?;
    state
        .require_known_bank(ctx.bank.as_str())
        .map_err(|error| CliError::from_am_error(&error))?;
    if verb != "summarize" {
        return Ok(ctx.bank.as_str().to_owned());
    }
    let Some(scope) = parsed.value("scope") else {
        return Ok(ctx.bank.as_str().to_owned());
    };
    if scope == "bank"
        || state
            .store
            .get_memory_item(ctx.bank.as_str(), scope)
            .map_err(|error| CliError::from_am_error(&error))?
            .is_some()
    {
        return Ok(ctx.bank.as_str().to_owned());
    }
    Err(CliError::operational(
        "item_not_found",
        format!("item `{scope}` not found in bank"),
    ))
}

/// The read-only pending-work counts a dry run reports next to its `would`
/// sentence. `summarize` counts the scope's items: `gists` the real call would
/// regenerate (active items holding a snapshot) and `skipped` the rest
/// (archived, discarded, no snapshot). `consolidate` counts the
/// `dirty_ancestors` its wave would refresh and the bank's existing
/// `consolidated_units` its reconcile pass visits.
///
/// Every query is a read the real call also runs; nothing is written and no
/// lock is waited for. A store failure fails the dry run the real call would.
///
/// # Errors
/// A store failure while reading the pending work.
pub(crate) fn preview_counts(
    state: &McpState,
    verb: &str,
    parsed: &ParsedArgs,
    bank: &str,
) -> Result<Value, CliError> {
    match verb {
        "summarize" => summarize_counts(state, parsed, bank),
        _ => consolidate_counts(state, parsed, bank),
    }
}

/// Page size for the read-only `inspect_items` walk (mirrors the walk the real
/// bank-scope summarize runs; any page size yields the same total).
const PREVIEW_PAGE: usize = 200;

/// The extraction-availability gate the real summarize call enforces before
/// any item work, run the same way against the runtime's effective config.
///
/// # Errors
/// The same `not_implemented` error the real call fails with when no gist
/// generator is configured.
fn extractor_gate(state: &McpState) -> Result<(), CliError> {
    let effective = {
        let rt = state.config.lock().expect("config runtime mutex poisoned");
        rt.effective_raw()
    };
    if extract_available_from(&effective) {
        return Ok(());
    }
    Err(CliError::from_am_error(&AmError::new(
        ErrorCode::NotImplemented,
        "extraction unavailable: no extractor configured (set extract.url to a local \
         extraction sidecar, or extract.provider=openai with a credential); no gist was \
         regenerated and no text was fabricated",
    )))
}

fn summarize_counts(state: &McpState, parsed: &ParsedArgs, bank: &str) -> Result<Value, CliError> {
    // The real call fails closed before touching any item when no gist
    // generator is configured; the dry run refuses the same way, so it never
    // previews success where the real run fails.
    extractor_gate(state)?;
    let mut gists = 0_u64;
    let mut skipped = 0_u64;
    match parsed.value("scope") {
        // Bank scope mirrors the real walk: every row reports, and only an
        // active item holding a snapshot gets its gist regenerated.
        Some("bank") => {
            let mut offset = 0_usize;
            loop {
                let mut filter = ItemInspectFilter::new(bank);
                filter.validity = InspectValidity::All;
                filter.include_discarded = true;
                filter.limit = PREVIEW_PAGE;
                filter.offset = offset;
                let rows = state
                    .store
                    .inspect_items(&filter)
                    .map_err(|error| CliError::from_am_error(&error))?;
                let full_page = rows.len() == PREVIEW_PAGE;
                for row in &rows {
                    if row.archived || row.discarded {
                        skipped += 1;
                        continue;
                    }
                    match state
                        .store
                        .get_memory_item(bank, &row.item_id)
                        .map_err(|error| CliError::from_am_error(&error))?
                    {
                        Some(item) if item.snapshot.is_some() => gists += 1,
                        _ => skipped += 1,
                    }
                }
                if !full_page {
                    break;
                }
                offset += PREVIEW_PAGE;
            }
        }
        // Item scope mirrors the real call: an archived or discarded item is
        // skipped, and only a snapshot feeds the extractor.
        Some(item_id) => {
            if state
                .store
                .item_is_archived(bank, item_id)
                .map_err(|error| CliError::from_am_error(&error))?
                || state
                    .store
                    .get_item_identity(bank, item_id)
                    .map_err(|error| CliError::from_am_error(&error))?
                    .is_none()
            {
                skipped = 1;
            } else {
                match state
                    .store
                    .get_memory_item(bank, item_id)
                    .map_err(|error| CliError::from_am_error(&error))?
                {
                    Some(item) if item.snapshot.is_some() => gists = 1,
                    _ => skipped = 1,
                }
            }
        }
        // Unreachable: `summarize` requires --scope at build time.
        None => {}
    }
    Ok(json!({"gists": gists, "skipped": skipped}))
}

fn consolidate_counts(
    state: &McpState,
    parsed: &ParsedArgs,
    bank: &str,
) -> Result<Value, CliError> {
    // The wave refreshes the dirty ancestors it finds; with `--force` it marks
    // every internal node first, so the count reads the tree it would dirty.
    let dirty = if parsed.has("force") {
        forced_dirty_ancestor_count(state, bank)
    } else {
        state.maint.try_dirty_ancestor_count(bank)
    };
    let units = state
        .store
        .list_distilled_units(bank)
        .map_err(|error| CliError::from_am_error(&error))?
        .len();
    Ok(json!({
        // `null` = the forest lock is held (contention), never zero dirty work.
        "dirty_ancestors": dirty.map_or(Value::Null, |count| json!(count)),
        "consolidated_units": units,
    }))
}

/// The internal-node count `--force` would dirty, read from the shared forest
/// without mutating it. `None` = the lock is held; never a fabricated count.
fn forced_dirty_ancestor_count(state: &McpState, bank: &str) -> Option<usize> {
    let forest = state.forest.try_lock().ok()?;
    let Some(tree) = forest.tree(bank) else {
        return Some(0);
    };
    let mut count = 0_usize;
    let mut queue: Vec<&str> = tree.root_id().into_iter().collect();
    while let Some(node_id) = queue.pop() {
        let Some(node) = tree.get(node_id) else {
            continue;
        };
        if node.depth > 0 {
            count += 1;
        }
        queue.extend(node.child_ids.iter().map(String::as_str));
    }
    Some(count)
}

#[cfg(test)]
#[path = "cli_write_preview_tests.rs"]
mod tests;
