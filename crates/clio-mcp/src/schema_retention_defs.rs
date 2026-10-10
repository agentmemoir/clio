// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for per-tool JSON Schema definitions for the
//! retention-profile tools and the batch admission preview.
//!
//! ## Owns
//! - `inputSchema` for `admit_preview_batch`, `retention_profile_get`, and
//!   `retention_profile_set`
//! - The batch candidate item schema and the published profile schema reference
//!
//! ## Does not own
//! - Core catalog definitions (see `schema_defs`)
//! - Tool dispatch or handler semantics (see `retention_tools`)
//!
//! ## Boundary
//!
//! Changes to a retention tool's parameter contract should normally require
//! changes here. Changes to pack packaging or dispatch should not require
//! changes here.
//!
//! Keep this module focused on retention tool schema definitions.

use crate::schema_defs::{ToolDef, b, bank_actor, en, store_like};
use serde_json::{Map, Value, json};

/// Candidate object accepted by one batch preview item: the single-item preview
/// arguments minus the connection-level `bank`/`actor`.
fn batch_candidate() -> Value {
    let mut properties = Map::new();
    for (name, schema) in store_like("admit_preview", "unused").props {
        if name == "bank" || name == "actor" {
            continue;
        }
        properties.insert(name.to_owned(), schema);
    }
    json!({
        "type": "object",
        "description": "One candidate scored exactly as a real write would be; `item` carries the content.",
        "properties": Value::Object(properties),
        "additionalProperties": false
    })
}

/// All retention-profile tool definitions.
#[must_use]
pub fn retention_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "admit_preview_batch",
            description: "Dry-run a batch of candidates under the bank profile: per-item decisions (each carrying `mission_rule_ids` when the retention mission fires) plus summary counts, writing nothing.",
            required: &["items"],
            props: {
                let mut props = vec![
                    (
                        "items",
                        json!({
                            "type": "array",
                            "description": "Candidates in input order (1..=50).",
                            "items": batch_candidate()
                        }),
                    ),
                    (
                        "profile_override",
                        json!({
                            "type": "object",
                            "description": "Preview a candidate profile without persisting it.",
                            "additionalProperties": false,
                            "properties": clio_config::retention_schema::profile_json_schema()["properties"].clone()
                        }),
                    ),
                ];
                props.extend(bank_actor());
                props
            },
        },
        ToolDef {
            name: "retention_profile_get",
            description: "Show the effective per-bank retention profile, its source, the optional retention mission (statement and example counts when set), and the fields stored but not yet honored.",
            required: &[],
            props: bank_actor(),
        },
        ToolDef {
            name: "retention_profile_set",
            description: "Validate and store a per-bank retention profile; without confirm=true it previews the change and writes nothing.",
            required: &["profile"],
            props: {
                let mut props = vec![
                    (
                        "profile",
                        json!({
                            "type": "object",
                            "description": "Retention profile object (verbosity, theta_offsets, duplicate tolerances, recall scope, retention curve and floor, pack fidelity, provenance floor, maintenance trigger, version) plus an optional mission: keep/drop examples that steer admission deterministically.",
                            "additionalProperties": false,
                            "properties": clio_config::retention_schema::profile_json_schema()["properties"].clone()
                        }),
                    ),
                    (
                        "scope",
                        en(
                            "Write scope; deployment sets the default for banks without an override.",
                            &["bank", "deployment"],
                        ),
                    ),
                    (
                        "confirm",
                        b("Confirm the profile change; without it nothing is stored."),
                    ),
                ];
                props.extend(bank_actor());
                props
            },
        },
    ]
}

#[cfg(test)]
#[path = "schema_retention_defs_tests.rs"]
mod schema_retention_defs_tests;
