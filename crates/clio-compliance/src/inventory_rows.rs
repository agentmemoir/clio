// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `RowPlan` schema of the compliance scope
//! ledger and for reconciling the fixed plan against what exists: which
//! planned tables the live store names or misses, and which in-scope source
//! areas the plan covers.
//!
//! ## Owns
//! - `RowPlan`, the schema of one planned row
//! - `pg_source_note`, the per-backend note for sources one backend cannot
//!   resolve
//! - Table-pattern matching between the plan and the measured store tables
//! - `REQUIRED_SOURCE_AREAS` and `uncovered_required_areas`
//! - The internal `inventory_map` and `inventory_plan` submodule declarations
//!
//! ## Does not own
//! - The fixed `ROW_PLAN` table itself (`inventory_plan`)
//! - Deciding whether a row is open (`scope_inventory` in the parent module)
//!
//! ## Boundary
//!
//! Changes to the row schema, to plan-to-store matching, or to which source
//! areas are required should normally require changes here. Changes to which
//! families the ledger reports should not.
//!
//! Keep this module focused on the row schema and plan reconciliation.

#[path = "inventory_map.rs"]
mod inventory_map;
#[path = "inventory_plan.rs"]
mod inventory_plan;

pub(crate) use self::inventory_plan::ROW_PLAN;

use crate::inventory::{Custody, Owner, Readability, RetentionAction, RowSource};

/// One row's fixed, backend-independent plan.
///
/// Fields: `family`, `tables`, `source`, `link`, `gap`, then the handling
/// cluster (`owner`, `custody`, `readability`, `key_path`, `action`) and the
/// `proof` that pins the row's facts.
pub(crate) struct RowPlan {
    pub(crate) family: &'static str,
    /// Store tables this row names; a trailing `*` matches a name prefix.
    pub(crate) tables: &'static [&'static str],
    pub(crate) source: RowSource,
    pub(crate) link: Option<&'static str>,
    /// The named gap when the link is absent, hashed, legacy, or unknown.
    pub(crate) gap: Option<&'static str>,
    pub(crate) owner: Owner,
    pub(crate) custody: Custody,
    pub(crate) readability: Readability,
    pub(crate) key_path: Option<&'static str>,
    pub(crate) action: RetentionAction,
    pub(crate) proof: &'static str,
}

/// The named per-backend handling of a planned source the Postgres read-only
/// target measurement cannot resolve: the extension-dependent vector table is
/// held by the store yet never enumerated, and the SQLite full-text shadow
/// tables have no Postgres counterpart. `None` for a source both backends can
/// resolve.
pub(crate) fn pg_source_note(pattern: &str) -> Option<&'static str> {
    match pattern {
        "item_embeddings*" => Some(
            "item_embeddings* is not enumerated by the read-only target measurement; not individually verified on this backend",
        ),
        "items_fts*" => {
            Some("items_fts* is a SQLite-only full-text source, not a Postgres relation")
        }
        _ => None,
    }
}

/// The in-scope source areas the ledger must always report, each paired with
/// the planned families that satisfy it. `uncovered_required_areas` reconciles
/// this list against `ROW_PLAN`, so a required area whose families all vanish
/// from the plan is named by the coverage check instead of silently dropping
/// out of the ledger.
// Used only by the coverage test, which asserts `uncovered_required_areas`
// stays empty.
#[allow(dead_code)]
pub(crate) const REQUIRED_SOURCE_AREAS: &[(&str, &[&str])] = &[
    (
        "stored families",
        &[
            "items",
            "triples",
            "assoc_edges",
            "persona_stable",
            "persona_preferences",
            "tasks",
            "failures",
            "beliefs",
            "tombstones",
        ],
    ),
    ("derivatives", &["derived_item_structures"]),
    ("indexes", &["derived_item_structures"]),
    ("exports", &["export_copies"]),
    ("stderr", &["stderr_events"]),
    ("events", &["events"]),
    ("archives", &["backup_archives"]),
    ("restore routes", &["restore_routes"]),
    ("enabled peers", &["enabled_peers"]),
];

/// The required source areas no planned row satisfies. Empty when `ROW_PLAN`
/// covers every required area; the coverage test asserts this stays empty.
// Used only by the coverage test; see `REQUIRED_SOURCE_AREAS`.
#[allow(dead_code)]
pub(crate) fn uncovered_required_areas() -> Vec<&'static str> {
    REQUIRED_SOURCE_AREAS
        .iter()
        .filter(|(_, families)| {
            !families
                .iter()
                .any(|family| ROW_PLAN.iter().any(|plan| plan.family == *family))
        })
        .map(|(area, _)| *area)
        .collect()
}

/// Whether `pattern` names `table`: an exact name, or a trailing-`*` prefix.
pub(crate) fn pattern_matches(pattern: &str, table: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => table.starts_with(prefix),
        None => pattern == table,
    }
}

/// Whether any planned row names `table`.
pub(crate) fn named_by_plan(table: &str) -> bool {
    ROW_PLAN.iter().any(|plan| {
        plan.tables
            .iter()
            .any(|pattern| pattern_matches(pattern, table))
    })
}

/// The planned tables the live store does not hold.
pub(crate) fn missing_tables<'a>(plan: &'a RowPlan, tables: &[(String, u64)]) -> Vec<&'a str> {
    plan.tables
        .iter()
        .filter(|pattern| {
            !tables
                .iter()
                .any(|(name, _)| pattern_matches(pattern, name))
        })
        .copied()
        .collect()
}
