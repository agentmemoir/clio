// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for per-tool JSON Schema definitions for the five
//! operator surfaces, published in the same
//! `tool_schema` pack as the Core catalog.
//!
//! ## Owns
//! - One `inputSchema` per operator tool: `diagnose`, `verify`, `doctor`,
//!   `repair`, `reindex`
//!
//! ## Does not own
//! - Core, additive, hygiene, or portability tool definitions
//! - Tool dispatch or handler semantics (`ops_tools`)
//!
//! ## Boundary
//!
//! Changes to an operator tool's parameter contract should normally require
//! changes here. Changes to operator check logic should not require changes
//! here.
//!
//! Keep this module focused on operator schema definitions.

use crate::schema_defs::{ToolDef, arr, b, en, s};

/// All operator tool definitions (three read-planning + two mutating rows).
#[must_use]
pub fn ops_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "diagnose",
            description: "Read-only whole-store install/database health snapshot: backend reachability, schema version, banks, index coverage counts, and embedding readiness. Never mutates memory and takes no bank scope.",
            required: &[],
            props: vec![("timestamp", s("Snapshot time; defaults to now."))],
        },
        ToolDef {
            name: "verify",
            description: "Deeper read-only consistency pass over referential integrity and index coverage; aggregate ok is false when any error-severity finding exists.",
            required: &[],
            props: vec![
                ("bank", s("Bank scope; whole store when omitted.")),
                ("timestamp", s("Report time; defaults to now.")),
            ],
        },
        ToolDef {
            name: "doctor",
            description: "Compose an ordered repair plan from verify findings. Default dry_run=true prints the plan and registers nothing; dry_run=false registers the plan and returns its id without touching the store.",
            required: &[],
            props: vec![
                ("bank", s("Bank scope; whole store when omitted.")),
                (
                    "dry_run",
                    b("Print the plan only (default true); false registers a concrete plan id."),
                ),
                ("timestamp", s("Composition time; defaults to now.")),
            ],
        },
        ToolDef {
            name: "repair",
            description: "Apply allowlisted repairs from a doctor plan id or an explicit action list. confirm=true is required to mutate; unknown actions are refused and successful runs re-verify and report residual findings.",
            required: &[],
            props: vec![
                ("plan_id", s("Doctor-returned plan id to apply.")),
                (
                    "actions",
                    arr(
                        "Explicit allowlisted action ids (used when plan_id is omitted).",
                        &s("One allowlisted action id."),
                    ),
                ),
                (
                    "bank",
                    s(
                        "Bank scope; required for explicit actions (or when the plan \
                         carries none). Without a bank every action fails with no \
                         mutation.",
                    ),
                ),
                (
                    "confirm",
                    b("Perform the repairs; default false (refused)."),
                ),
                ("timestamp", s("Repair time; defaults to now.")),
            ],
        },
        ToolDef {
            name: "reindex",
            description: "Rebuild derived dense and/or lexical indexes. dry_run=true (default) reports scope and cost only; a mutating run requires confirm=true and an embedding sidecar for the dense leg.",
            required: &[],
            props: vec![
                (
                    "target",
                    en(
                        "Which derived indexes to rebuild (default all).",
                        &["dense", "lexical", "all"],
                    ),
                ),
                (
                    "bank",
                    s(
                        "Bank scope; every indexable bank when omitted (rejected over \
                         MCP while the shared bank surface is disabled).",
                    ),
                ),
                ("dry_run", b("Report scope/cost only (default true).")),
                ("confirm", b("Perform the rebuild; default false.")),
                ("timestamp", s("Report time; defaults to now.")),
            ],
        },
    ]
}
