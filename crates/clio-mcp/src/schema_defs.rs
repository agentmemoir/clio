// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for per-tool JSON Schema definitions for the
//! normative Core tool catalog, the single source of truth for the
//! published `tool_schema` pack.
//!
//! ## Owns
//! - One `inputSchema` per Core tool (MCP tool rules: root `type: object`)
//! - Enum vocabulary constants (`epistemic_kind`, `update_rule`, categories,
//!   episodic type tags) as separate properties
//!
//! ## Does not own
//! - Pack assembly, hash stamping, or export (see `schema`)
//! - Tool dispatch or handler semantics (see `write_tools`)
//!
//! ## Boundary
//!
//! Changes to a tool's parameter contract should normally require changes here.
//! Changes to pack packaging or dispatch should not require changes here.
//!
//! Keep this module focused on per-tool schema definitions.

use serde_json::{Value, json};

/// Semantic categories, sourced from `clio-types` so the published enum
/// cannot drift from the domain whitelist.
pub const SEMANTIC_CATEGORIES: &[&str] = clio_types::SemanticCategory::WIRE;

/// Episodic type tags (never a sixth semantic category).
pub const EPISODIC_TYPES: [&str; 5] = ["triple", "gist", "task", "failure", "temporal"];

/// Epistemic kind of a fact or belief item.
pub const EPISTEMIC_KINDS: [&str; 2] = ["fact", "belief"];

/// Update rules for mutators.
pub const UPDATE_RULES: [&str; 2] = ["discrete", "continuous"];

/// Provenance source types.
pub const SOURCE_TYPES: [&str; 3] = ["user_stated", "agent_inferred", "third_party"];

/// Time axes for bi-temporal reads.
pub const TIME_AXES: [&str; 2] = ["valid", "transaction"];

/// Retrieve/compose domains.
pub const DOMAINS: [&str; 6] = [
    "semantic", "episodic", "task", "failure", "temporal", "persona",
];

/// One published tool: MCP `tools/list` entry shape.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    /// Normative tool name.
    pub name: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Required property names.
    pub required: &'static [&'static str],
    /// Ordered `(property name, JSON Schema)` pairs.
    pub props: Vec<(&'static str, Value)>,
}

impl ToolDef {
    /// Render as an MCP tool entry with `inputSchema`.
    #[must_use]
    pub fn to_tool(&self) -> Value {
        let mut properties = serde_json::Map::new();
        for (name, schema) in &self.props {
            properties.insert((*name).to_owned(), schema.clone());
        }
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": {
                "type": "object",
                "properties": Value::Object(properties),
                "required": self.required,
                "additionalProperties": false
            }
        })
    }
}

pub(crate) fn s(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}

pub(crate) fn en(desc: &str, values: &[&str]) -> Value {
    json!({"type": "string", "description": desc, "enum": values})
}

pub(crate) fn num(desc: &str) -> Value {
    json!({"type": "number", "description": desc})
}

pub(crate) fn b(desc: &str) -> Value {
    json!({"type": "boolean", "description": desc})
}

pub(crate) fn arr(desc: &str, items: &Value) -> Value {
    json!({"type": "array", "description": desc, "items": items.clone()})
}

pub(crate) fn bank_actor() -> Vec<(&'static str, Value)> {
    vec![
        (
            "bank",
            s("Isolation key; overrides the connection default when present."),
        ),
        (
            "actor",
            en(
                "Attribution actor.",
                &["agent", "harness", "user", "system"],
            ),
        ),
    ]
}

/// Candidate `item` shell for `admit_preview` / `store`.
fn item_shell() -> Value {
    json!({
        "type": "object",
        "description": "Candidate item shell (content lives in snapshot/gist).",
        "properties": {
            "id": s("Optional caller item id; generated when omitted."),
            "subject_id": s("Data-subject id for DEK selection; defaults to the item id."),
            "snapshot": {
                "type": "object",
                "description": "Authoritative structured snapshot (requires source_text when present). It is span-verified before commit, so on a verifying tool it may carry only the non-empty leaves entity, amount, and date; any other non-empty leaf is rejected. Put other content in gist."
            },
            "gist": s("Non-authoritative prose gist.")
        },
        "additionalProperties": false
    })
}

/// The bounded optional `context` parameter shared by every tool that stores
/// context-bearing content (same declaration on all six domain-record writes
/// and the item-level writes).
pub(crate) fn context_prop() -> (&'static str, Value) {
    (
        "context",
        json!({
            "type": "string",
            "maxLength": 4096,
            "description": "Optional bounded descriptive source context, recording where the memory came from, such as a conversation type or workflow stage. It is not searchable: recall does not match on it, so put anything you want to find later in the item text or gist. At most 4096 UTF-8 bytes; rejected when empty or oversized, never truncated."
        }),
    )
}

/// The `allow_duplicate` parameter shared by every gated write except the item
/// family, which words its own description. It waives the exact-duplicate
/// rejection and nothing else.
pub(crate) fn allow_duplicate_prop() -> (&'static str, Value) {
    (
        "allow_duplicate",
        b(
            "Store even when identical to an item already in this bank; waives the exact-duplicate rejection only.",
        ),
    )
}

pub(crate) fn store_like(name: &'static str, desc: &'static str) -> ToolDef {
    let mut props = vec![
        ("item", item_shell()),
        (
            "category",
            en(
                "Semantic category; required for semantic writes.",
                SEMANTIC_CATEGORIES,
            ),
        ),
        (
            "episodic_type",
            en(
                "Episodic type tag; used instead of category for episodic writes.",
                &EPISODIC_TYPES,
            ),
        ),
        ("epistemic_kind", en("Fact or belief.", &EPISTEMIC_KINDS)),
        ("source_type", en("Provenance source type.", &SOURCE_TYPES)),
        ("confidence", num("Optional confidence in 0.0..=1.0.")),
        (
            "evidence_ref",
            s("Provenance alias; copied into item source_ref when unset."),
        ),
        (
            "source_text",
            s("Span haystack; required when snapshot is present."),
        ),
        context_prop(),
    ];
    if name == "store" {
        props.push(("dry_run", b("Validate and score without durable write.")));
    }
    // Every tool in this family can hit the unconditional exact-duplicate
    // rejection, so every tool in this family must be able to waive it. Naming
    // the argument for what it waives keeps it distinct from `consolidate`'s
    // unrelated `force`.
    props.push((
        "allow_duplicate",
        b("Store this item even when its text is identical to an item already in this bank. Waives the exact-duplicate rejection only; every other gate still applies."),
    ));
    props.extend(bank_actor());
    ToolDef {
        name,
        description: desc,
        required: &["item", "epistemic_kind"],
        props,
    }
}

/// All Core tool definitions (write tools plus the read tools published here
/// and bound on the shared dispatcher by `read_tools`).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn core_defs() -> Vec<ToolDef> {
    let mut defs = vec![
        store_like(
            "admit_preview",
            "Run category + five-factor scoring; returns the decision (carrying `mission_rule_ids` when the retention mission fires) without writing.",
        ),
        store_like(
            "store",
            "Admit a long-term item through structural, span-verify, taxonomy, and admission gates; the decision carries `mission_rule_ids` when the retention mission fires.",
        ),
        ToolDef {
            name: "update",
            description: "Route an update by rule: `discrete` invalidates, `continuous` applies EMA.",
            required: &["target", "new_value", "update_rule"],
            props: vec![
                (
                    "target",
                    s("Item id (discrete) or declared attribute key (continuous)."),
                ),
                (
                    "new_value",
                    s("New value; numeric for the continuous path."),
                ),
                ("update_rule", en("Which rule to apply.", &UPDATE_RULES)),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "invalidate",
            description: "Close the item's validity on both time axes; never deletes.",
            required: &["item_id"],
            props: vec![
                ("item_id", s("Item to invalidate.")),
                ("replacement_id", s("Optional superseding item id.")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "discard",
            description: "Logged operations-removal from active use; dry-run without `confirm`.",
            required: &["item_id", "reason"],
            props: vec![
                ("item_id", s("Item to remove from active use.")),
                (
                    "reason",
                    s("Non-empty operator-supplied removal reason (never logged verbatim)."),
                ),
                (
                    "confirm",
                    b("Perform the removal; default false (dry-run preview)."),
                ),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "consolidate",
            description: "Trigger the maintenance wave: dirty-path refresh, eligible \
hub distillation, and reconcile of the bank's consolidated units; never blocks \
leaf writes. The same structural maintenance also runs automatically: the \
runtime's maintenance trigger checks each bank's pressure and fires when that \
pressure crosses the thresholds, subject to the cooldown; the thresholds and \
cooldown are tunable. This tool remains the deterministic, caller-invoked path.",
            required: &[],
            // `bank_actor` supplies the shared `bank`/`actor` pair; the later
            // `bank` entry replaces the helper's generic wording with this
            // tool's selector wording (`to_tool` inserts in order, last wins).
            props: vec![(
                "force",
                b("Run the full forced maintenance wave: refresh every dirty \
                     ancestor, then reconcile the bank's consolidated units."),
            )]
            .into_iter()
            .chain(bank_actor())
            .chain([(
                "bank",
                s("Target bank for the maintenance wave; defaults to the injected bank."),
            )])
            .collect(),
        },
        ToolDef {
            name: "graph_link",
            description: "Declare an explicit association edge; identity includes the relationship label.",
            required: &["source_id", "target_id", "relationship"],
            props: vec![
                ("source_id", s("Source item id (must exist in the bank).")),
                ("target_id", s("Target item id (must exist in the bank).")),
                ("relationship", s("Explicit relationship label.")),
                ("weight", num("Optional initial weight (default 1.0).")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
    ];
    defs.extend(crate::schema_record_defs::record_defs());
    defs.extend(crate::schema_read_defs::read_defs());
    defs
}

#[cfg(test)]
#[path = "schema_defs_tests.rs"]
mod schema_defs_tests;
