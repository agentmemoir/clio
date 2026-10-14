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
//! - `command_bindings` and `catalog_rows`, which resolve each spine key to
//!   its registry record
//!
//! ## Does not own
//! - The command records themselves (see the `cli_command_registry_*` modules)
//! - The record model (see `cli_command_spec`) or the rendering of either
//!   surface (`cli_help` and `command_catalog`)
//!
//! ## Boundary
//!
//! Changes to the order either surface prints should normally require changes
//! here. A command's row moving into or out of a registry record should not:
//! its key is already in the spine, and the record carries the row.
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
/// not hold fails closed at render time.
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
/// Every key resolves to its registry record, so the spine and the registry
/// must list the same commands; the tests beside this module pin that in both
/// directions.
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
    "compliance inventory",
    "compliance review-export",
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

/// The CLI command → MCP tool binding, in whole-catalog order.
///
/// Every key resolves to its registry record, and the fail-closed panic keeps
/// a spine typo from silently dropping a command from the surface.
pub(crate) fn command_bindings() -> Vec<(&'static str, &'static str)> {
    BINDING_ORDER
        .iter()
        .map(|key| {
            let spec = crate::cli_command_registry::find(key)
                .unwrap_or_else(|| panic!("`{key}` has no binding record"));
            let tool = spec
                .tool
                .unwrap_or_else(|| panic!("`{key}` binds no MCP tool"));
            (spec.name, tool)
        })
        .collect()
}

/// Every catalog row in whole-catalog order.
///
/// Every key resolves to its registry record's name and summary, and the
/// fail-closed panic keeps a spine typo from silently dropping a row from the
/// catalog.
pub(crate) fn catalog_rows() -> Vec<(&'static str, &'static str)> {
    CATALOG_ORDER
        .iter()
        .map(|key| {
            let spec = crate::cli_command_registry::find(key)
                .unwrap_or_else(|| panic!("`{key}` has no catalog row"));
            (spec.name, spec.summary)
        })
        .collect()
}

#[cfg(test)]
#[path = "cli_command_bindings_tests.rs"]
mod tests;
