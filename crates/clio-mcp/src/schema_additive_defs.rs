// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for per-tool JSON Schema definitions for the
//! additive harness workspace tools, published in the same
//! `tool_schema` pack as the Core catalog.
//!
//! ## Owns
//! - One `inputSchema` per additive tool in this slice (`batch`, `scratchpad_*`,
//!   `canonical_*`, `shared_*`, `validate`, `stats`)
//!
//! ## Does not own
//! - Core tool definitions (see `schema_defs`)
//! - Tool dispatch or handler semantics
//!
//! ## Boundary
//!
//! Changes to an additive tool's parameter contract should normally require
//! changes here. Changes to core schemas should not require changes here.
//!
//! Keep this module focused on additive schema definitions.

use crate::schema_defs::{
    EPISTEMIC_KINDS, SEMANTIC_CATEGORIES, SOURCE_TYPES, ToolDef, allow_duplicate_prop, b,
    bank_actor, context_prop, en, num, s,
};
use serde_json::Value;

/// One batch operation element schema.
fn batch_op() -> Value {
    serde_json::json!({
        "type": "object",
        "description": "One ordered write operation: {tool, params}.",
        "properties": {
            "tool": {
                "type": "string",
                "description": "Recognized mutating Core tool.",
                "enum": ["store", "triple_add", "canonical_put", "persona_put_stable",
                          "task_upsert", "failure_record", "graph_link"]
            },
            "params": {"type": "object", "description": "Arguments of the wrapped tool; `$ref:<index>.id` resolves to an earlier op's item id. `store` and `canonical_put` params accept the bounded optional `context` field documented on those tools."}
        },
        "required": ["tool"],
        "additionalProperties": false
    })
}

fn scratchpad_like(
    name: &'static str,
    desc: &'static str,
    required: &'static [&'static str],
) -> crate::schema_defs::ToolDef {
    let mut props = vec![("key", s("Scratchpad key."))];
    if name == "scratchpad_write" {
        props.push((
            "content",
            s("Content string or JSON (stores under key; overwrites)."),
        ));
    }
    props.push((
        "session",
        s("Session scope (defaults to the connection session)."),
    ));
    props.extend(bank_actor());
    ToolDef {
        name,
        description: desc,
        required,
        props,
    }
}

/// All additive tool definitions (write + read rows of this slice).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn additive_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "batch",
            description: "Execute Core write mutations atomically in ONE transaction; any admission failure rolls everything back.",
            required: &["operations"],
            props: vec![
                (
                    "operations",
                    serde_json::json!({
                        "type": "array",
                        "description": "Ordered operations (max 50); $ref:<index>.id resolves earlier ops' item ids.",
                        "items": batch_op()
                    }),
                ),
                ("dry_run", b("Score and validate all operations without writing.")),
            ],
        },
        scratchpad_like(
            "scratchpad_write",
            "Store process-local working notes; data is lost when the MCP process exits. Never durable or indexed (64 KB / 100-key caps).",
            &["key", "content"],
        ),
        scratchpad_like(
            "scratchpad_read",
            "Read one scratchpad key, or the whole session scratchpad when key is omitted.",
            &[],
        ),
        scratchpad_like(
            "scratchpad_clear",
            "Drop one scratchpad key, or purge the session scratchpad when key is omitted.",
            &[],
        ),
        ToolDef {
            name: "canonical_put",
            description: "Single-slot upsert: closes the prior value on both time axes and inserts the admitted revision.",
            required: &["key", "value", "category", "epistemic_kind"],
            props: vec![
                ("key", s("Canonical slot key (e.g. primary_database_host).")),
                ("value", s("Current slot value (prose).")),
                (
                    "snapshot",
                    serde_json::json!({
                        "type": "object",
                        "description": "Optional authoritative structured snapshot (verified against source_text). It may carry only the non-empty leaves entity, amount, and date; any other non-empty leaf is rejected."
                    }),
                ),
                ("source_text", s("Span haystack for snapshot verification; required with a snapshot and rejected without one.")),
                ("evidence_ref", s("Optional evidence reference.")),
                ("category", en("Semantic category of the slot value.", SEMANTIC_CATEGORIES)),
                ("epistemic_kind", en("Fact or belief.", &EPISTEMIC_KINDS)),
                ("source_type", en("Provenance source type.", &SOURCE_TYPES)),
                ("confidence", num("Optional confidence in 0.0..=1.0.")),
                context_prop(),
                ("dry_run", b("Score and validate without writing.")),
                (
                    "item",
                    serde_json::json!({
                        "type": "object",
                        "description": "Optional shell carrying a caller item id.",
                        "properties": {"id": s("Caller item id; generated when omitted.")},
                        "additionalProperties": false
                    }),
                ),
                allow_duplicate_prop(),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "canonical_get",
            description: "Read the currently active slot value, or a historical value at `as_of`.",
            required: &["key"],
            props: vec![
                ("key", s("Canonical slot key.")),
                ("as_of", s("Point-in-time (transaction axis) for historical reads.")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "shared_store",
            description: "Store an admitted item in the cross-agent shared bank (bank forced to `shared`).",
            required: &["item", "epistemic_kind", "actor"],
            props: vec![
                (
                    "item",
                    serde_json::json!({
                        "type": "object",
                        "description": "Candidate item shell (content lives in snapshot/gist).",
                        "properties": {
                            "id": s("Optional caller item id; generated when omitted."),
                            "subject_id": s("Data-subject id for DEK selection; defaults to the item id."),
                            "snapshot": {"type": "object"},
                            "gist": s("Non-authoritative prose gist.")
                        },
                        "additionalProperties": false
                    }),
                ),
                ("category", en("Semantic category.", SEMANTIC_CATEGORIES)),
                ("episodic_type", en("Episodic type tag.", &["triple", "gist", "task", "failure", "temporal"])),
                ("epistemic_kind", en("Fact or belief.", &EPISTEMIC_KINDS)),
                ("source_type", en("Provenance source type.", &SOURCE_TYPES)),
                ("confidence", num("Optional confidence.")),
                ("evidence_ref", s("Provenance alias.")),
                ("source_text", s("Span haystack for snapshot verification; required with a snapshot and rejected without one.")),
                context_prop(),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "shared_retrieve",
            description: "Hybrid search scoped exclusively to the shared bank.",
            required: &["query", "actor"],
            props: vec![("query", s("Search text."))]
                .into_iter()
                .chain(bank_actor())
                .collect(),
        },
        ToolDef {
            name: "shared_discard",
            description: "Confirmed operations removal from the shared bank (dry-run without confirm).",
            required: &["item_id", "reason", "actor"],
            props: vec![
                ("item_id", s("Item to remove from active use.")),
                ("reason", s("Non-empty removal reason.")),
                ("confirm", b("Perform the removal; default false (preview).")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "validate",
            description: "Reviewer loop: attest, update (auditable correction), invalidate, or discard a memory item.",
            required: &["item_id", "action"],
            props: vec![
                ("item_id", s("Target memory item id.")),
                (
                    "action",
                    en(
                        "Reviewer action; `delete` is accepted as an alias for `discard`.",
                        &["attest", "update", "invalidate", "discard", "delete"],
                    ),
                ),
                (
                    "note",
                    s("Reviewer observation; required non-empty for update/invalidate/discard."),
                ),
                ("new_content", s("Corrected value; required for `update`.")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "stats",
            description: "Read-only memory counts by bank, tier, category, age band, and provenance class. Two lists: `by_age_band` buckets active items by age in half-life units (the unit is the retention half-life in days, and each band's boundaries in days appear in its label), and `by_provenance_class` counts user_stated, agent_inferred, third_party, and fact. Pressure counts (flagged, stale summaries, archive candidates) appear when their sources answer, and a count that cannot be answered is left out rather than shown as zero. Tier is the semantic-versus-episodic split (semantic is category-scoped, episodic is the remainder) and nothing else.",
            required: &[],
            props: bank_actor(),
        },
    ]
}
