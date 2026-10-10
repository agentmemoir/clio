// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the persona, history, triple, and belief
//! record-write tool definitions in the published schema (`persona_put_stable`,
//! `persona_observe_preference`, `task_upsert`, `failure_record`, `triple_add`,
//! `triple_end`, and `belief_observe`).
//!
//! ## Owns
//! - `record_defs`: tool declarations for the record-write surface
//!
//! ## Does not own
//! - Item-level write tool definitions (`schema_defs`)
//! - Read tool definitions (`schema_read_defs`)
//!
//! ## Boundary
//!
//! Changes to record-write tool declarations should normally require changes
//! here. Changes to the item write surface should not require changes here.
//!
//! Keep this module focused on record-write tool declarations.

use crate::schema_defs::{
    EPISTEMIC_KINDS, SOURCE_TYPES, ToolDef, UPDATE_RULES, arr, b, bank_actor, context_prop, en,
    num, s,
};

/// Persona, history, triple, and belief record-write tool declarations.
#[must_use]
#[allow(clippy::too_many_lines)]
pub(crate) fn record_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "persona_put_stable",
            description: "Upsert a discrete/categorical stable persona entry under invalidation semantics.",
            required: &["key", "value"],
            props: vec![
                ("key", s("Stable attribute key.")),
                (
                    "value",
                    s("Categorical value (numeric payloads are rejected)."),
                ),
                ("confidence", num("Optional confidence in 0.0..=1.0.")),
                (
                    "source",
                    en(
                        "`explicit` or `inferred` (default explicit).",
                        &["explicit", "inferred"],
                    ),
                ),
                ("evidence_ref", s("Evidence reference.")),
                context_prop(),
                ("allow_duplicate", b("Store even when identical to an item already in this bank; waives the exact-duplicate rejection only.")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "persona_observe_preference",
            description: "EMA-update a continuous scalar preference intensity.",
            required: &["key", "observed_value"],
            props: vec![
                ("key", s("Preference key.")),
                ("observed_value", num("Observed scalar intensity (finite).")),
                ("confidence", num("Optional confidence in 0.0..=1.0.")),
                (
                    "source",
                    en(
                        "`explicit` or `inferred` (default inferred).",
                        &["explicit", "inferred"],
                    ),
                ),
                ("evidence_ref", s("Evidence reference.")),
                context_prop(),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "task_upsert",
            description: "Create a gated task-history version.",
            required: &["task_id", "definition", "status"],
            props: vec![
                ("task_id", s("Caller task identifier.")),
                ("definition", s("Task definition (required).")),
                ("steps", arr("Ordered steps taken so far.", &s("One step."))),
                ("status", s("Free-form status label (required).")),
                ("evidence_ref", s("Optional evidence reference.")),
                context_prop(),
                ("allow_duplicate", b("Store even when identical to an item already in this bank; waives the exact-duplicate rejection only.")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "failure_record",
            description: "Store a structured, gated failure record with a length-capped lesson.",
            required: &["task_id", "attempt_n", "what_failed", "lesson"],
            props: vec![
                ("task_id", s("Task this failure belongs to.")),
                ("attempt_n", num("1-based attempt number.")),
                ("what_failed", s("What went wrong (never truncated).")),
                (
                    "lesson",
                    s("Natural-language lesson (rejected over the token cap)."),
                ),
                ("evidence_ref", s("Optional evidence reference.")),
                context_prop(),
                ("allow_duplicate", b("Store even when identical to an item already in this bank; waives the exact-duplicate rejection only.")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "triple_add",
            description: "Write a bi-temporal SPO edge through episodic admission; default supersede closes prior open subject+predicate.",
            required: &["subject", "predicate", "object", "epistemic_kind"],
            props: vec![
                ("subject", s("SPO subject.")),
                ("predicate", s("SPO predicate.")),
                ("object", s("SPO object payload.")),
                ("epistemic_kind", en("Fact or belief.", &EPISTEMIC_KINDS)),
                ("source_type", en("Required iff belief.", &SOURCE_TYPES)),
                (
                    "valid_from",
                    s("Optional valid-time start (defaults to now)."),
                ),
                ("valid_until", s("Optional valid-time end.")),
                ("confidence", num("Optional confidence.")),
                (
                    "supersede",
                    b("Close prior open subject+predicate (default true)."),
                ),
                ("snapshot_ref", s("Optional snapshot item ref.")),
                ("gist_ref", s("Optional gist item ref.")),
                (
                    "subject_id",
                    s("Data-subject id for DEK selection; defaults to the subject."),
                ),
                (
                    "update_rule",
                    en("Must be `discrete` when present.", &UPDATE_RULES),
                ),
                context_prop(),
                ("allow_duplicate", b("Store even when identical to an item already in this bank; waives the exact-duplicate rejection only.")),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "triple_end",
            description: "Expire open SPO edges without replacement; history is retained.",
            required: &["subject", "predicate", "valid_until"],
            props: vec![
                ("subject", s("SPO subject.")),
                ("predicate", s("SPO predicate.")),
                (
                    "object",
                    s("Optional object filter; ends only matching edges when present."),
                ),
                ("valid_until", s("Expiration timestamp.")),
                (
                    "update_rule",
                    en("Must be `discrete` when present.", &UPDATE_RULES),
                ),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        ToolDef {
            name: "belief_observe",
            description: "Append a confidence-history entry; create the belief (gated) when the identity is new.",
            required: &["proposition", "confidence", "evidence_ref", "source_type"],
            props: vec![
                (
                    "proposition",
                    s("Proposition text; identity is the normalized proposition per bank."),
                ),
                ("confidence", num("Observed confidence in 0.0..=1.0.")),
                (
                    "evidence_ref",
                    s("Evidence reference for this observation."),
                ),
                ("source_type", en("Provenance source type.", &SOURCE_TYPES)),
                ("as_of", s("Optional observation time; defaults to now.")),
                (
                    "subject_id",
                    s("Data-subject id for DEK selection; defaults to a proposition-derived id."),
                ),
                context_prop(),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
    ]
}
