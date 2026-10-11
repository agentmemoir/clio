// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the single explicit global order of the two
//! rendered help surfaces: the command → MCP tool bindings behind
//! `clio help --json`, and the catalog rows behind `clio help`.
//!
//! ## Owns
//! - `BINDING_ORDER` and `CATALOG_ORDER`, the verb-keyed order spines of the
//!   two surfaces
//! - `command_bindings` and `catalog_rows`, which resolve each spine key
//!   record-first and fall back to the row the surface still holds statically
//! - The binding rows that have not moved into the registry yet
//!
//! ## Does not own
//! - The command records themselves (see the `cli_command_registry_*` modules)
//! - The authored catalog rows (see `command_catalog`)
//! - The record model (see `cli_command_spec`) or the rendering of either
//!   surface (`cli_help` and `command_catalog`)
//!
//! ## Boundary
//!
//! Changes to the order either surface prints, or to the binding rows below,
//! should normally require changes here. A command moving out of an authored
//! row into a registry record should not: its key is already in the spine.
//!
//! Keep this module focused on the global order and its resolution.
//

/// The binding surface's key order: every command that binds an MCP tool, in
/// the binding surface's own published order, exactly as `command_bindings`
/// prints them.
///
/// This spine intentionally differs from [`CATALOG_ORDER`]: the binding
/// surface and the catalog surface are separate help outputs, each with its
/// own order. Do not align the two spines; that would change the command
/// order of `clio help --json`.
///
/// A registered command's key resolves to its record; a key the registry does
/// not hold resolves to its row in [`STATIC_BINDINGS`].
pub(crate) static BINDING_ORDER: &[&str] = &[
    "recall",
    "get",
    "show",
    "inspect",
    "stats",
    "remember",
    "admit",
    "summarize",
    "consolidate",
    "update",
    "invalidate",
    "discard",
    "correct",
    "task get",
    "task history",
    "failure list",
    "memtree query",
    "memtree get",
    "history temporal",
    "triple query",
    "triple add",
    "triple end",
    "belief history",
    "belief observe",
    "graph query",
    "graph link",
    "associations",
    "persona get",
    "persona stable",
    "persona observe",
    "task upsert",
    "failure record",
    "scratchpad write",
    "scratchpad read",
    "scratchpad clear",
    "canonical put",
    "canonical get",
    "shared store",
    "shared retrieve",
    "shared discard",
    "validate",
    "batch",
    "maintenance status",
    "intent gate",
    "compose-context",
    "audit trail",
    "hygiene audit",
    "hygiene clean",
    "hygiene log",
    "export",
    "import",
    "erase",
    "config get",
    "config set",
    "config profiles",
    "config use",
    "ranking get",
    "ranking set",
    "sync status",
    "sync push",
    "sync pull",
    "sync serve",
    "sync key status",
];

/// The catalog surface's key order: the whole catalog, exactly as
/// `catalog_rows` prints it.
///
/// A registered command's key resolves to its record; a key the registry does
/// not hold resolves to its authored row in `command_catalog`. The two sources
/// interleave, because the records print where the rows they replaced printed.
pub(crate) static CATALOG_ORDER: &[&str] = &[
    "version",
    "health",
    "mcp schema-export",
    "mcp stdio",
    "mcp http",
    "ops diagnose",
    "ops verify",
    "ops doctor",
    "ops repair",
    "ops reindex",
    "status",
    "compose up",
    "compose down",
    "setup",
    "retention schema",
    "retention get",
    "retention set",
    "recall",
    "get",
    "show",
    "inspect",
    "stats",
    "task get",
    "task history",
    "failure list",
    "memtree query",
    "memtree get",
    "history temporal",
    "triple query",
    "belief history",
    "graph query",
    "associations",
    "persona get",
    "persona stable",
    "persona observe",
    "task upsert",
    "failure record",
    "scratchpad write",
    "scratchpad read",
    "scratchpad clear",
    "canonical put",
    "canonical get",
    "shared store",
    "shared retrieve",
    "shared discard",
    "validate",
    "batch",
    "maintenance status",
    "intent gate",
    "compose-context",
    "audit trail",
    "hygiene audit",
    "hygiene clean",
    "hygiene log",
    "export",
    "import",
    "erase",
    "remember",
    "admit",
    "summarize",
    "consolidate",
    "update",
    "invalidate",
    "discard",
    "correct",
    "triple add",
    "triple end",
    "belief observe",
    "graph link",
    "config get",
    "config set",
    "config profiles",
    "config use",
    "ranking get",
    "ranking set",
    "sync status",
    "sync push",
    "sync pull",
    "sync serve",
    "sync keygen",
    "sync key export",
    "sync key status",
    "backup dir",
    "backup create",
    "backup list",
    "backup prune",
    "backup restore",
    "backup verify",
    "auth token add",
    "auth token list",
    "auth token revoke",
    "install",
    "uninstall",
    "help",
];

/// The CLI command → MCP tool bindings not yet held in the command registry,
/// in whole-catalog order. Two-token commands use a space-separated key.
///
/// Every key here is already in [`BINDING_ORDER`]; this table is only the row
/// an un-moved command prints, and the row is deleted as the command moves
/// into a registry record.
static STATIC_BINDINGS: &[(&str, &str)] = &[
    ("remember", "store"),
    ("admit", "admit_preview"),
    ("summarize", "summarize"),
    ("consolidate", "consolidate"),
    ("update", "update"),
    ("invalidate", "invalidate"),
    ("discard", "discard"),
    ("correct", "correct"),
    ("validate", "validate"),
    ("batch", "batch"),
    ("export", "export"),
    ("import", "import"),
    ("erase", "erase_request"),
    ("config get", "config_get"),
    ("config set", "config_set"),
    ("config profiles", "config_profiles"),
    ("config use", "config_profile_apply"),
    ("ranking get", "ranking_env_get"),
    ("ranking set", "ranking_env_set"),
    ("sync status", "sync_status"),
    ("sync push", "sync_push"),
    ("sync pull", "sync_pull"),
    ("sync serve", "sync_serve"),
    ("sync key status", "sync_key_status"),
];

/// The CLI command → MCP tool binding, in whole-catalog order.
///
/// Each key resolves record-first: a registered record that binds a tool is
/// the row, and the still-static row is the fallback while the move is
/// incomplete. Every key has exactly one row, and the fail-closed panic keeps
/// a spine typo from silently dropping a command from the surface.
pub(crate) fn command_bindings() -> Vec<(&'static str, &'static str)> {
    BINDING_ORDER
        .iter()
        .map(|key| binding_row(key).unwrap_or_else(|| panic!("`{key}` has no binding row")))
        .collect()
}

/// The binding row for one spine key: the record's tool, else the static row.
///
/// The static table holds one row per un-moved command, and the test suite
/// pins that no key is served by both sources.
fn binding_row(key: &str) -> Option<(&'static str, &'static str)> {
    if let Some(spec) = crate::cli_command_registry::find(key)
        && let Some(tool) = spec.tool
    {
        return Some((spec.name, tool));
    }
    static_row(STATIC_BINDINGS, key)
}

/// Every catalog row in whole-catalog order.
///
/// Each key resolves record-first: a registered record prints its name and
/// summary, and the authored row is the fallback while the move is
/// incomplete. Every key has exactly one row, and the fail-closed panic keeps
/// a spine typo from silently dropping a row from the catalog.
pub(crate) fn catalog_rows() -> Vec<(&'static str, &'static str)> {
    CATALOG_ORDER
        .iter()
        .map(|key| catalog_row(key).unwrap_or_else(|| panic!("`{key}` has no catalog row")))
        .collect()
}

/// The catalog row for one spine key: the record, else the authored row.
fn catalog_row(key: &str) -> Option<(&'static str, &'static str)> {
    if let Some(spec) = crate::cli_command_registry::find(key) {
        return Some((spec.name, spec.summary));
    }
    crate::command_catalog::ENTRIES_BEFORE
        .iter()
        .chain(crate::command_catalog::ENTRIES_AFTER.iter())
        .find(|(name, _)| *name == key)
        .copied()
}

/// The row for `key` in a still-static table.
fn static_row(
    table: &'static [(&'static str, &'static str)],
    key: &str,
) -> Option<(&'static str, &'static str)> {
    table.iter().find(|(name, _)| *name == key).copied()
}

#[cfg(test)]
#[path = "cli_command_bindings_tests.rs"]
mod tests;
