// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for per-tool JSON Schema definitions for the
//! Effective-config and ranking-environment tools.
//!
//! ## Owns
//! - `inputSchema` for `config_get`, `config_set`, `config_profiles`,
//!   `config_profile_apply`, `ranking_env_get`, and `ranking_env_set`
//!
//! ## Does not own
//! - Core catalog definitions (see `schema_defs`)
//! - Tool dispatch or handler semantics (see `config_tools`)
//!
//! ## Boundary
//!
//! Changes to a config/ranking tool's parameter contract should normally
//! require changes here. Changes to config behavior should not require
//! changes here.
//!
//! Keep this module focused on config/ranking tool schema definitions.

use crate::schema_defs::{ToolDef, b, s};
use serde_json::json;

/// Config-change scopes (`config_set`).
pub const CONFIG_SCOPES: [&str; 3] = ["session", "profile", "deployment"];

/// All config/ranking tool definitions.
#[must_use]
pub fn config_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "config_get",
            description: "Show the effective configuration after defaults, files, env, and runtime overlays; secrets are masked.",
            required: &[],
            props: vec![(
                "path",
                s("Optional dotted config path; omitted shows the whole document."),
            )],
        },
        ToolDef {
            name: "config_set",
            description: "Validate and change a config value at session, profile, or deployment scope; secret values are echoed masked.",
            required: &["path", "value"],
            props: vec![
                ("path", s("Dotted config path to set.")),
                (
                    "value",
                    json!({"description": "New value; types and ranges are validated before apply."}),
                ),
                (
                    "scope",
                    json!({
                        "type": "string",
                        "description": "Where the change applies (default session).",
                        "enum": CONFIG_SCOPES,
                        "default": "session"
                    }),
                ),
            ],
        },
        ToolDef {
            name: "config_profiles",
            description: "List named profiles and the active one.",
            required: &[],
            props: vec![],
        },
        ToolDef {
            name: "config_profile_apply",
            description: "Activate a profile in this server process's config session and return the effective-config diff; the change is visible to config views for this server process for its lifetime and this tool does not write it anywhere, while `clio config use` on the command line persists the same profile for later commands, and a successful apply pushes the profile's ranking environment into the already-open retrieval snapshot so the next retrieval uses it, while the admission policy and the intent gate stay on the ranking environment fixed at process open. Admission and category gates are never bypassed.",
            required: &["name"],
            props: vec![("name", s("Profile name to activate."))],
        },
        ToolDef {
            name: "ranking_env_get",
            description: "Show the effective hybrid ranking and admission environment for this process's config session: retrieval weights, admission weights, co-activation constants, intent-gate thresholds, the retention shaping group (`retention.half_life_days`, default 30, the age at which the decaying share of the floored age factor is halved), and the maintenance-trigger group (`maintenance_triggers`: `dirty_ancestor_threshold`, `eligible_hub_threshold`, and `stale_unit_threshold` each default 1 and trigger a wave when their count is greater than or equal to the threshold, with `0` disabling just that one condition; `cooldown_seconds` defaults 300 and at `0` means no cooldown; `max_waves_per_hour` defaults 12 and rejects `0`; `hub_budget_per_wave` defaults 16 and rejects `0`, because a zero budget would spend a wave and distil nothing).",
            required: &[],
            props: vec![],
        },
        ToolDef {
            name: "ranking_env_set",
            description: "Patch the ranking/admission environment for this server process's config session; the patch is readable back through `ranking_env_get` and `config_get` for this server process's lifetime and this tool does not write it anywhere, while `clio ranking set` on the command line persists the same environment for later commands, and the patch governs the next retrieval: a successful set pushes the new retrieval weights, retention half-life, trust ratios, and co-activation constants into the already-open retrieval snapshot and its association engine, so a `coactivation` patch reaches the association hub, the retriever's own query expansion, and the graph tools, while the admission policy, the intent gate, and the hygiene weights stay on the ranking environment fixed at process open. `dry_run=true` returns the normalized weights that would apply. Weights that must sum to 1.0 are renormalized or rejected with a clear error.",
            required: &["patch"],
            props: vec![
                (
                    "patch",
                    json!({
                        "type": "object",
                        "description": "Ranking/admission fields to change (retrieval, admission, coactivation, intent_gate, theta_admit, theta_overrides, type_prior_overrides, hygiene, retention, trust, maintenance_triggers). `maintenance_triggers` patches `dirty_ancestor_threshold`, `eligible_hub_threshold`, and `stale_unit_threshold` (each default 1, firing a wave when the count is greater than or equal to the threshold; `0` disables just that one condition), plus `cooldown_seconds` (default 300; `0` means no cooldown), `max_waves_per_hour` (default 12; rejects `0`), and `hub_budget_per_wave` (default 16; rejects `0`, because a zero budget would spend a wave and distil nothing).",
                        "additionalProperties": true
                    }),
                ),
                (
                    "dry_run",
                    b("Validate and report the normalized environment without applying."),
                ),
            ],
        },
    ]
}

#[cfg(test)]
#[path = "schema_config_defs_tests.rs"]
mod schema_config_defs_tests;
