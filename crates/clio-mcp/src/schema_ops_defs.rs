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

/// The omitted-`bank` wording `verify`, `doctor` and `reindex` share: the
/// argument then means the server default bank, and the credential's grant has
/// to cover that bank or the call is refused. A caller without a credential has
/// no default to fall back on, so each of those tools states that reach itself,
/// while `diagnose` publishes its own sentence.
const OMITTED_BANK: &str =
    "Bank scope; omitted means the server default bank, which the credential's grant must cover.";

/// `diagnose`'s own `bank` clause: the shared omitted-bank sentence, then the
/// reach only this tool keeps, since its arm reads the argument without ever
/// narrowing an unbound caller to it.
fn diagnose_bank_clause() -> String {
    format!(
        "{OMITTED_BANK} A caller without a credential reads the whole store, whether it names \
         this or not."
    )
}

/// The `repair` scope, which is the plan's when a plan is registered, so the
/// grant has to cover the scope the tool acts on and not only the argument. A
/// plan outside the grant is answered as not found, in the words below, and an
/// unbound caller keeps the plan-or-argument reach it had before the grant.
const REPAIR_BANK: &str = "Bank scope; omitted means the registered plan's own bank, and \
                           otherwise the bank this call resolves for the credential (the server \
                           default when no `bank` is passed). A plan registered for a bank the \
                           credential's grant does not cover is reported as not found, and none \
                           of its actions run. A caller without a credential acts in its `bank` \
                           argument when it passes one, and with neither a bank nor a registered \
                           plan has no bank scope at all, where each action fails without mutating.";

/// All operator tool definitions (three read-planning + two mutating rows).
#[must_use]
pub fn ops_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "diagnose",
            description: "Read-only install/database health snapshot: backend reachability, \
                          schema version, banks, index coverage counts, and embedding \
                          readiness. Never mutates memory. A bound caller's `bank`, when it \
                          passes one, selects the bank inside its grant, and a caller without \
                          a credential gets the whole store whatever it passes.",
            required: &[],
            props: vec![
                ("bank", s(&diagnose_bank_clause())),
                ("timestamp", s("Snapshot time; defaults to now.")),
            ],
        },
        ToolDef {
            name: "verify",
            description: "Deeper read-only consistency pass over referential integrity and index coverage; aggregate ok is false when any error-severity finding exists.",
            required: &[],
            props: vec![
                (
                    "bank",
                    s(&format!(
                        "{OMITTED_BANK} A caller without a credential reads the whole store \
                         when it omits this."
                    )),
                ),
                ("timestamp", s("Report time; defaults to now.")),
            ],
        },
        ToolDef {
            name: "doctor",
            description: "Compose an ordered repair plan from verify findings. Default dry_run=true prints the plan and registers nothing; dry_run=false registers the plan and returns its id without touching the store.",
            required: &[],
            props: vec![
                (
                    "bank",
                    s(&format!(
                        "{OMITTED_BANK} A caller without a credential composes over the whole \
                         store when it omits this."
                    )),
                ),
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
                ("bank", s(REPAIR_BANK)),
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
                    s(&format!(
                        "{OMITTED_BANK} A caller without a credential sweeps every indexable \
                         bank when it omits this (rejected over MCP while the shared bank \
                         surface is disabled)."
                    )),
                ),
                ("dry_run", b("Report scope/cost only (default true).")),
                ("confirm", b("Perform the rebuild; default false.")),
                ("timestamp", s("Report time; defaults to now.")),
            ],
        },
    ]
}

#[cfg(test)]
#[path = "schema_ops_defs_tests.rs"]
mod schema_ops_defs_tests;
