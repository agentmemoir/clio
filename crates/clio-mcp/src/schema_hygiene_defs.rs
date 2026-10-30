// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for per-tool JSON Schema definitions for the
//! hygiene tools, published in the same `tool_schema`
//! pack as the Core catalog.
//!
//! ## Owns
//! - One `inputSchema` per hygiene tool: `hygiene_audit`, `hygiene_clean`,
//!   `hygiene_log_list`
//!
//! ## Does not own
//! - Core or additive tool definitions (`schema_defs` / `schema_additive_defs`)
//! - Tool dispatch or handler semantics
//!
//! ## Boundary
//!
//! Changes to a hygiene tool's parameter contract should normally require
//! changes here. Changes to hygiene service behavior should not require
//! changes here.
//!
//! Keep this module focused on hygiene schema definitions.

use crate::schema_defs::{ToolDef, b, bank_actor, en, num, s};

/// All hygiene tool definitions (one mutating + two read rows).
#[must_use]
pub fn hygiene_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "hygiene_audit",
            description: "Read-only ranked noise scan: candidates by descending hygiene_noise_score with masked previews; never mutates memory.",
            required: &[],
            props: vec![
                ("min_score", num("Rank floor in 0.0..=1.0 (default 0.40).")),
                ("limit", num("Ranked page size, 1..=200 (default 50).")),
                ("offset", num("Offset into the ranked candidate list.")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "hygiene_clean",
            description: "Confirmed cleanup: flag | archive | discard (delete alias, non-compliance) | apply_suggested (apply each candidate's suggested_action; a keep suggestion is skipped). Without confirm=true: dry-run counts only, zero writes. Never performs compliance erasure or DEK destruction.",
            required: &[],
            props: vec![
                (
                    "candidates",
                    serde_json::json!({
                        "type": "array",
                        "description": "Prior hygiene_audit candidates (item_id + noise_score + suggested_action).",
                        "items": {
                            "type": "object",
                            "properties": {
                                "item_id": {"type": "string"},
                                "noise_score": {"type": "number"},
                                "suggested_action": {"type": "string",
                                    "enum": ["flag", "archive", "discard"]}
                            },
                            "required": ["item_id"],
                            "additionalProperties": false
                        }
                    }),
                ),
                (
                    "item_ids",
                    serde_json::json!({
                        "type": "array",
                        "description": "Bare item ids (alternative to candidates).",
                        "items": {"type": "string"}
                    }),
                ),
                (
                    "action",
                    en(
                        "Requested action; `delete` is the documented non-compliance alias of `discard`.",
                        &["flag", "archive", "discard", "delete", "apply_suggested"],
                    ),
                ),
                ("confirm", b("Perform the mutations; default false (dry-run counts only).")),
                ("reason", s("Non-empty removal reason for confirmed discard.")),
                ("note", s("Review note persisted on flag.")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "hygiene_log_list",
            description: "Masked durable hygiene-log rows (one row per item per clean action); every cleanup is reconstructible without secret leakage.",
            required: &[],
            props: vec![
                ("limit", num("Page size, 1..=200 (default 50).")),
                ("offset", num("Page offset.")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
    ]
}
