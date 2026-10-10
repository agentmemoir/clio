// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MemTree agent-facing tools: status, consolidate trigger, get, query.
//!
//! ## Owns
//! - `maintenance_status` response assembly and dirty/refreshing reporting
//! - `consolidate` accept flow (non-blocking background wave or wait mode)
//! - `memtree_get` / `memtree_query` bank-scoped reads
//!
//! ## Does not own
//! - Dirty-set coalescing or the refresh engine (`memtree_maint`)
//! - Tree topology / attach / split algorithms
//!
//! ## Boundary
//!
//! Changes to tool request/response shapes should normally require changes here.
//! Changes to refresh scheduling should not require changes here.
//!
//! Keep this module focused on MemTree inspect/trigger tool handlers.

use crate::hub_distill::HubDistillEngine;
use crate::memtree::MemTreeQueryFilter;
use crate::memtree_maint::{MemTreeMaint, SharedMemForest};
use clio_types::{AmError, ErrorCode, OpContext, ToolResult};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;

/// `maintenance_status` response.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MaintenanceStatusData {
    /// Leaf remains queryable via leaf index / get-by-id.
    pub leaf_queryable: bool,
    /// Elapsed time of the latest completed structural refresh wave (ms).
    ///
    /// This is zero until the first wave completes; later waves retain the
    /// previous completed value while a new wave is queued or running.
    pub structural_maintenance_ms: u128,
    /// Elapsed time through the latest leaf-publish boundary (ms).
    pub time_to_queryable_ms: u128,
    /// Ancestor node ids currently dirty.
    pub dirty_ancestors: Vec<String>,
    /// Ancestor node ids currently refreshing.
    pub refreshing: Vec<String>,
    /// Optional last error from a failed refresh.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Optional item id when scoped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
}

/// `consolidate` accept response (non-blocking by default).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConsolidateAccept {
    /// Accepted job id.
    pub job_id: String,
    /// `accepted` | `completed` (when the caller joins the wave).
    pub status: String,
    /// Hub distillation outcome: the real `report.summary()` from the live call
    /// when the wave ran, `hub_distill:pending` when it was queued in the
    /// background, and `not_implemented` when no hub engine or context was
    /// supplied to the call.
    pub hubs: String,
    /// Nodes cleared in this wave (empty when async).
    pub refreshed: Vec<String>,
}

/// Run `maintenance_status(item_id?)`.
pub fn maintenance_status(
    maint: &MemTreeMaint,
    bank_id: &str,
    item_id: Option<&str>,
) -> ToolResult<MaintenanceStatusData> {
    if bank_id.is_empty() {
        return ToolResult::failure(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    let structural_maintenance_ms = maint.structural_maintenance_ms(bank_id);
    let time_to_queryable_ms = maint.leaves.time_to_queryable_ms(bank_id);
    let forest = maint.forest.lock().expect("forest");
    let tree = forest.tree(bank_id);
    let leaf_queryable = match item_id {
        Some(id) => maint.leaves.get(bank_id, id).is_some(),
        None => true,
    };
    let (dirty_ancestors, refreshing, last_error) = match (tree, item_id) {
        (Some(t), Some(id)) => {
            let leaf_nid = t.leaf_node_id_for_item(id);
            let ancestors = leaf_nid.map(|nid| t.ancestors_of(nid)).unwrap_or_default();
            let mut dirty = Vec::new();
            let mut refreshing = Vec::new();
            let mut last_error = None;
            for aid in &ancestors {
                if let Some(n) = t.get(aid) {
                    if n.dirty {
                        dirty.push(aid.clone());
                    }
                    if n.refreshing {
                        refreshing.push(aid.clone());
                    }
                    if last_error.is_none() {
                        last_error.clone_from(&n.last_error);
                    }
                }
            }
            (dirty, refreshing, last_error)
        }
        (Some(t), None) => {
            let dirty = t.dirty_node_ids();
            let mut refreshing = Vec::new();
            let mut last_error = None;
            for n in t.query(&MemTreeQueryFilter {
                limit: Some(1000),
                ..Default::default()
            }) {
                if n.refreshing {
                    refreshing.push(n.id.clone());
                }
                if last_error.is_none() {
                    last_error = n.last_error;
                }
            }
            (dirty, refreshing, last_error)
        }
        (None, _) => (vec![], vec![], None),
    };
    ToolResult::success(MaintenanceStatusData {
        leaf_queryable,
        structural_maintenance_ms,
        time_to_queryable_ms,
        dirty_ancestors,
        refreshing,
        last_error,
        item_id: item_id.map(str::to_owned),
    })
}

/// Trigger dirty-path refresh plus eligible `hub_distill` jobs.
///
/// `wait=true` runs the wave on this thread and returns the real result
/// (`status: completed`, the real `refreshed` list, the real hub summary).
/// `wait=false` queues it on a detached thread and returns immediately, which
/// is only honest for a process that outlives the wave; a one-shot caller must
/// pass `wait=true` or the thread dies with the process and the command reports
/// work it never did.
///
/// Pass `hub` and `ctx` to run hub distillation under `retention` (the bank's
/// resolved retention profile); without them the response reports
/// `not_implemented`.
pub fn consolidate_tool(
    maint: &Arc<MemTreeMaint>,
    hub: Option<&Arc<HubDistillEngine>>,
    ctx: Option<&OpContext>,
    bank_id: &str,
    retention: clio_config::retention::ResolvedRetention,
    force: bool,
    wait: bool,
) -> ToolResult<ConsolidateAccept> {
    if bank_id.is_empty() {
        return ToolResult::failure(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if force {
        let mut forest = maint.forest.lock().expect("forest");
        let ids = forest.tree_mut(bank_id).force_dirty_all_internal();
        drop(forest);
        maint.enqueue_dirty(bank_id, &ids);
    }
    let job_id = format!("job-{}", maint.jobs.fetch_add(1, Ordering::Relaxed));
    if wait {
        let refreshed = maint.refresh_bank(bank_id);
        // A caller that joined the wave is entitled to the result of the work it
        // paid for. Reporting `completed` with an error string inside `hubs`
        // would be a success shape over a wave that did not finish.
        return match hubs_text(hub, ctx, bank_id, &retention) {
            Ok(hubs) => ToolResult::success(ConsolidateAccept {
                job_id,
                status: "completed".into(),
                hubs,
                refreshed,
            }),
            Err(error) => ToolResult::failure(error),
        };
    }
    let maint_bg = Arc::clone(maint);
    let bank = bank_id.to_owned();
    let hub_bg = hub.map(Arc::clone);
    let ctx_bg = ctx.cloned();
    // Claim the in-flight slot before spawning, not inside the new thread. A
    // waiter that reads the gauge between the spawn and the thread's first
    // instruction would see an idle queue while this wave is still pending.
    maint.begin_wave();
    thread::spawn(move || {
        let _ = maint_bg.refresh_bank_inner(&bank);
        maint_bg.end_wave();
        // Nobody is left to report this wave's failure to: the response was
        // already sent as `accepted`. The wave's own audit rows are the record.
        let _ = hubs_text(hub_bg.as_ref(), ctx_bg.as_ref(), &bank, &retention);
    });
    let hubs = if hub.is_some() && ctx.is_some() {
        "hub_distill:pending".to_owned()
    } else {
        "not_implemented".to_owned()
    };
    ToolResult::success(ConsolidateAccept {
        job_id,
        status: "accepted".into(),
        hubs,
        refreshed: vec![],
    })
}

/// Run hub distillation and render a compact status string.
///
/// `Ok` carries the wave's real summary. A wave that could not run is an
/// `Err`, so a caller that joined the wave can report the failure instead of a
/// success shape over missing work.
fn hubs_text(
    hub: Option<&Arc<HubDistillEngine>>,
    ctx: Option<&OpContext>,
    bank_id: &str,
    retention: &clio_config::retention::ResolvedRetention,
) -> Result<String, AmError> {
    match (hub, ctx) {
        (Some(engine), Some(ctx)) => {
            Ok(engine.consolidate_hubs(bank_id, ctx, retention)?.summary())
        }
        _ => Ok("not_implemented".to_owned()),
    }
}

/// `memtree_get(node_id)` bank-scoped.
pub fn memtree_get(
    forest: &SharedMemForest,
    bank_id: &str,
    node_id: &str,
) -> ToolResult<crate::memtree::MemTreeNode> {
    if bank_id.is_empty() {
        return ToolResult::failure(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    let guard = forest.lock().expect("forest");
    match guard.tree(bank_id).and_then(|t| t.get(node_id)).cloned() {
        Some(n) => ToolResult::success(n),
        None => ToolResult::failure(AmError::new(
            ErrorCode::UnknownPath,
            format!("memtree node `{node_id}` not found in bank"),
        )),
    }
}

/// `memtree_query` bank-scoped.
pub fn memtree_query(
    forest: &SharedMemForest,
    bank_id: &str,
    filter: &MemTreeQueryFilter,
) -> ToolResult<Vec<crate::memtree::MemTreeNode>> {
    if bank_id.is_empty() {
        return ToolResult::failure(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    let guard = forest.lock().expect("forest");
    let nodes = match guard.tree(bank_id) {
        Some(t) => t.query(filter),
        None => vec![],
    };
    ToolResult::success(nodes)
}
