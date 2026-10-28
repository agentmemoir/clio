// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for schema-published read tool definitions
//! (bound on the shared dispatcher by `read_tools`).
//!
//! ## Owns
//! - `inputSchema` definitions for read/retrieve/compose/inspect Core tools
//! - The `retrieve` and `compose_context` row descriptions: the per-hit score
//!   contracts and the new bounded rendering, plus the pack channel's forms,
//!   its narrative-only warning, its shared budget, and the marked fallback
//! - The `retrieve` read controls whose contract is one short line, including
//!   the temporal-window `query_anchor`
//!
//! ## Does not own
//! - The long published property blocks (`schema_read_props` owns the read
//!   controls whose contract needs more than a line)
//! - Write-set definitions (see `schema_defs`)
//! - Pack assembly or dispatch
//!
//! ## Boundary
//!
//! Changes to a read tool's parameter contract should normally require changes here.
//! Changes to pack packaging or write tools should not require changes here.
//!
//! Keep this module focused on read-tool schema definitions.

use crate::schema_defs::{DOMAINS, TIME_AXES, ToolDef, arr, b, context_prop, en, num, s};
use crate::schema_read_props::{
    entity_summary_prop, fidelity_prop, include_entities_prop, include_exact_prop, min_scores_prop,
    provenance_floor_prop,
};
use serde_json::{Value, json};
/// Schema-published read tools (bound on the shared dispatcher by `read_tools`).
#[allow(clippy::too_many_lines)]
pub(crate) fn read_defs() -> Vec<ToolDef> {
    let read = |name: &'static str,
                desc: &'static str,
                required: &'static [&'static str],
                props: Vec<(&'static str, Value)>| {
        let mut props = props;
        // Common parameters, present on every read tool (additive).
        props.push((
            "bank",
            s("Isolation key; overrides the connection default when present."),
        ));
        props.push((
            "actor",
            en(
                "Attribution actor.",
                &["agent", "harness", "user", "system"],
            ),
        ));
        props.push(("explain", b("Return a PII-safe score/domain/hop trace.")));
        ToolDef {
            name,
            description: desc,
            required,
            props,
        }
    };
    vec![
        read(
            "get",
            "Exact fetch by id (metadata + refs). A soft-deleted item is still returned, marked by two top-level booleans: `discarded` (operations removal) and `archived` (hygiene archive). A hub-distilled unit also carries a `freshness` key: its proof count, last reconciliation, stale state and reason, and the per-revision evidence trail (source ids and times only).",
            &["item_id"],
            vec![("item_id", s("Item id."))],
        ),
        read(
            "get_snapshot",
            "Return the authoritative structured snapshot only. It also carries two top-level booleans: `discarded` (operations removal) and `archived` (hygiene archive).",
            &["item_id"],
            vec![("item_id", s("Item id."))],
        ),
        read(
            "get_gist",
            "Return the non-authoritative prose gist only. It also carries two top-level booleans: `discarded` (operations removal) and `archived` (hygiene archive).",
            &["item_id"],
            vec![("item_id", s("Item id."))],
        ),
        read(
            "summarize",
            "Regenerate the prose gist for one item (`scope` = item id) or every active item in the resolved bank (`scope` = \"bank\") from existing snapshots; snapshots are never altered. Archived and discarded items are reported as skipped, never summarized.",
            &["scope"],
            vec![
                (
                    "scope",
                    s("Item id, or the literal string \"bank\" for the whole resolved bank."),
                ),
                (
                    "dry_run",
                    b("Report the gists that would be regenerated without persisting them."),
                ),
            ],
        ),
        read(
            "maintenance_status",
            "Report leaf queryability and dirty/refreshing ancestors, plus a `pressure` block: \
             counts of dirty and refreshing ancestors, eligible hubs, pending and retry index \
             jobs, stale distilled units with the source that answered and the last completed \
             wave, and archive candidates. Each pressure entry is a count, or an unavailable \
             reason when its source is absent or fails (no hub engine, failed hub eligibility \
             scan, failed drain status read, failed stored-unit read, or failed archive scan). \
             Archive candidates are an on-demand bounded scan — at most 256 items per band scan, \
             two band scans — over the hygiene archive band (score 0.70 to below 0.90; \
             discard-band items such as secret hits are never counted), and `incomplete` marks a \
             partial scan. The `trigger` block carries the automatic maintenance trigger's last \
             recorded decision for the bank: the evaluated pressure counts, whether the last \
             evaluation fired a wave, the elapsed seconds since the last automatic wave and until \
             the next one is eligible (the trigger keeps monotonic instants, so no wall-clock time \
             is available for them), the waves in the last hour, and the last decline reason; a \
             bank the trigger has not evaluated renders an explicit unavailable marker. The \
             `index` pipeline block reuses the same one drain-status read; when that read fails, \
             the `index` block renders an unavailable value and the call still succeeds.",
            &[],
            vec![
                ("item_id", s("Optional item id.")),
                ("timestamp", s("Status time; defaults to now.")),
            ],
        ),
        read(
            "intent_gate",
            "Return whether retrieval is needed for this turn.",
            &["turn_text"],
            vec![("turn_text", s("Turn text to classify."))],
        ),
        read(
            "retrieve",
            "Adaptive dense + lexical + optional graph retrieval. A hit may carry `entity_match`, \
             a derived display-only inference that the query text mentions one of the hit's \
             `entities[]` names under the documented containment rule; it is not a verification \
             result, not a span-verified fact, and not a ranking signal. Each hit's \
             `scores.age_factor` is the retention factor applied to `scores.final`: a pure number \
             derived from timestamps and constants, in the bank's `[retention_floor, 1]` range \
             (exactly `1` when the curve is off), and never null for a returned hit. Each hit's \
             `scores.trust_factor` is the trust multiplier applied to `scores.final` at the \
             shaping site: a finite number in `[the smallest configured trust ratio, 1.0]`, always \
             present, that only reorders hits and never changes `category`, `epistemic_kind`, or \
             `confidence`; it is not a confidence value. A hit may also carry `exact_render`, a \
             bounded rendering of its authoritative half: it is content, it is absent unless the \
             request asks for it, and it never substitutes for the `get_snapshot` read of the \
             exact stored value.",
            &["query"],
            vec![
                ("query", s("Natural-language query.")),
                ("domains", arr("Domain filter.", &en("A domain.", &DOMAINS))),
                ("limit", num("Optional hit cap.")),
                ("as_of", s("Optional point-in-time read.")),
                (
                    "time_axis",
                    en("Bi-temporal axis (default valid).", &TIME_AXES),
                ),
                (
                    "query_anchor",
                    s(
                        "Optional RFC 3339 anchor (explicit UTC offset) for relative temporal expressions such as \"last week\"; without it the temporal arm does not run.",
                    ),
                ),
                ("expand_graph", b("Optional graph expansion.")),
                ("budget_tokens", num("Optional injection budget.")),
                (
                    "recall_scope",
                    en(
                        "Recall scope: full (default) or consolidated_only.",
                        &["full", "consolidated_only"],
                    ),
                ),
                (
                    "prefer_consolidated",
                    b(
                        "Drop raw items whose consolidated parent is also returned; freed slots backfill.",
                    ),
                ),
                ("min_scores", min_scores_prop()),
                ("provenance_floor", provenance_floor_prop()),
                ("include_entities", include_entities_prop()),
                ("entity_summary", entity_summary_prop()),
                ("include_exact", include_exact_prop()),
            ],
        ),
        read(
            "compose_context",
            "Return the bounded context pack under the token budget. Each \
             included hit contributes one form: flow (default) is the narrative \
             channel and must not be the only source of a numeric or \
             identity-bearing claim, so read `get_snapshot` for the exact \
             stored value; exact prefers each hit's bounded rendering, charged \
             against this same budget_tokens. A hit with no rendering \
             contributes its gist instead; a hit with neither a rendering nor a \
             gist is marked as a reference rather than silently replaced by its \
             identifier, and so is a hit whose rendering did not fit the \
             remaining budget. The pack's per-form tally counts every form.",
            &["budget_tokens"],
            vec![
                ("query", s("Optional query.")),
                ("domains", arr("Domain filter.", &en("A domain.", &DOMAINS))),
                ("budget_tokens", num("Token budget.")),
                ("as_of", s("Optional point-in-time read.")),
                (
                    "time_axis",
                    en("Bi-temporal axis (default valid).", &TIME_AXES),
                ),
                (
                    "query_anchor",
                    s(
                        "Optional RFC 3339 anchor (explicit UTC offset) for relative temporal expressions such as \"last week\"; without it the temporal arm does not run.",
                    ),
                ),
                ("expand_graph", b("Optional graph expansion.")),
                (
                    "recall_scope",
                    en(
                        "Memory-section scope: full (default) or consolidated_only.",
                        &["full", "consolidated_only"],
                    ),
                ),
                (
                    "prefer_consolidated",
                    b(
                        "Drop raw items whose consolidated parent is also returned in the memory section.",
                    ),
                ),
                ("fidelity", fidelity_prop()),
            ],
        ),
        read(
            "associations",
            "List co-activation/explicit edges with effective weights.",
            &["item_id"],
            vec![
                ("item_id", s("Item whose neighbors are listed.")),
                ("min_weight", num("Optional minimum effective weight.")),
            ],
        ),
        read(
            "persona_get",
            "Return the current bounded persona object, including optional source context per entry.",
            &[],
            vec![("budget_tokens", num("Optional persona budget."))],
        ),
        read(
            "task_get",
            "Fetch a task record, including its optional source context.",
            &["task_id"],
            vec![("task_id", s("Task id."))],
        ),
        read(
            "task_history",
            "Prior attempts/steps for a task.",
            &["task_id"],
            vec![
                ("task_id", s("Task id.")),
                ("limit", num("Optional limit.")),
            ],
        ),
        read(
            "failures_for_task",
            "Prior failures for the same or similar task.",
            &["task_id_or_query"],
            vec![
                ("task_id_or_query", s("Task id or free-text query.")),
                ("limit", num("Optional limit.")),
            ],
        ),
        read(
            "memtree_query",
            "Query MemTree leaves/ancestor summaries.",
            &[],
            vec![
                (
                    "time_range",
                    json!({
                        "type": "object",
                        "description": "Optional created_at window.",
                        "properties": {
                            "start": s("Inclusive lower bound."),
                            "end": s("Inclusive upper bound.")
                        },
                        "additionalProperties": false
                    }),
                ),
                ("task_id", s("Optional task id.")),
                ("depth", num("Optional depth cap.")),
                ("limit", num("Optional limit.")),
            ],
        ),
        read(
            "memtree_get",
            "Fetch one MemTree node.",
            &["node_id"],
            vec![("node_id", s("Node id."))],
        ),
        read(
            "temporal_history",
            "Trajectory for a fact, preference trend, or belief, with optional source context per record.",
            &["target"],
            vec![
                ("target", s("Target item/attribute/belief.")),
                ("as_of", s("Optional point-in-time.")),
                (
                    "time_axis",
                    en("Bi-temporal axis (default valid).", &TIME_AXES),
                ),
            ],
        ),
        read(
            "triple_query",
            "Pattern query over bi-temporal SPO edges.",
            &[],
            vec![
                ("subject", s("Optional subject.")),
                ("predicate", s("Optional predicate.")),
                ("object", s("Optional object.")),
                ("as_of", s("Optional point-in-time.")),
                (
                    "time_axis",
                    en("Bi-temporal axis (default valid).", &TIME_AXES),
                ),
                ("limit", num("Optional page size (default 50, max 200).")),
                ("offset", num("Optional page offset.")),
            ],
        ),
        read(
            "belief_history",
            "Full confidence trajectory for a belief, with the belief's optional source context.",
            &["belief_id_or_proposition"],
            vec![(
                "belief_id_or_proposition",
                s("Belief id or proposition text."),
            )],
        ),
        read(
            "graph_query",
            "Bounded multi-hop traversal over association edges.",
            &["seed_id"],
            vec![
                ("seed_id", s("Seed item id.")),
                ("max_hops", num("Optional hop cap.")),
                (
                    "edge_type",
                    en("Association kind filter.", &["coactivation", "explicit"]),
                ),
                ("min_weight", num("Optional minimum effective weight.")),
            ],
        ),
        read(
            "inspect",
            "List items with epistemic_kind, scores, validity windows, and optional source context for authorized readers (never payloads/DEKs).",
            &[],
            vec![
                (
                    "filter",
                    json!({
                        "type": "object",
                        "description": "Optional structured item filter (all fields optional).",
                        "properties": {
                            "category": s("Semantic category filter."),
                            "epistemic_kind": en("Fact or belief.", &["fact", "belief"]),
                            "validity": en(
                                "Validity-state filter.",
                                &["all", "active", "invalidated", "discarded"],
                            ),
                            "time_start": s("Inclusive lower bound on created_at."),
                            "time_end": s("Inclusive upper bound on created_at."),
                            "include_discarded": b("Include operationally discarded items.")
                        },
                        "additionalProperties": false
                    }),
                ),
                ("limit", num("Optional page size (default 50, max 200).")),
                ("offset", num("Optional page offset.")),
                (
                    "item_id",
                    s(
                        "Optional item id: when present, inspect adds a read-only supersession section for this item",
                    ),
                ),
                (
                    "query",
                    s(
                        "Required when item_id is present: the query the supersession check ranks the correction's lineage against",
                    ),
                ),
            ],
        ),
        read(
            "correct",
            "Auditable correction via update/invalidate rules; dry-run without `confirm`.",
            &["item_id", "reason"],
            vec![
                (
                    "item_id",
                    s(
                        "Item to correct (or declared attribute key for continuous; a belief id/proposition routes to the authorized context-only correction).",
                    ),
                ),
                (
                    "new_value",
                    // No `type`: a non-string value is serialized, not refused.
                    json!({
                        "description": "Corrected value for item/attribute corrections (a JSON object also replaces the structured snapshot). Required for item and attribute corrections; MUST be omitted for belief context-only corrections, which supply `context` instead — a non-empty `new_value` on a belief correction is rejected (values evolve only via `belief_observe`).",
                    }),
                ),
                (
                    "reason",
                    s("Mandatory non-empty audit reason (never a discard_reason)."),
                ),
                context_prop(),
                (
                    "confirm",
                    b("Perform the correction; default false (dry-run preview)."),
                ),
            ],
        ),
        read(
            "audit_trail",
            "Reconstruct value/confidence/evidence history with lineage traversal.",
            &["item_id"],
            vec![("item_id", s("Item id (any revision in the lineage)."))],
        ),
        read(
            "erase_request",
            "Compliance crypto-shred request path (never aliases discard).",
            &["subject_id", "legal_basis", "request_id"],
            vec![
                ("subject_id", s("Data subject to erase.")),
                ("legal_basis", s("Legal basis for the request.")),
                ("request_id", s("Caller request id.")),
                (
                    "reason_code",
                    s("Optional machine reason code (default COMPLIANCE_CRYPTO_SHRED)."),
                ),
                (
                    "confirm",
                    b("Perform the erasure; default false (unconfirmed calls never mutate)."),
                ),
            ],
        ),
    ]
}

#[cfg(test)]
#[path = "schema_read_defs_tests.rs"]
mod schema_read_defs_tests;

#[cfg(test)]
#[path = "schema_read_defs_exact_tests.rs"]
mod schema_read_defs_exact_tests;
