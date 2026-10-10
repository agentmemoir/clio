// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for the published JSON Schema of the retention
//! profile object, including the mission extension and mapping tables.
//!
//! ## Owns
//! - `profile_json_schema()` with the exact mapping tables, mission fields, and
//!   the inline mission-entry schema
//! - `schema_refs_resolve()`, the `$ref` pointer-resolution check used by
//!   schema-pack tests
//!
//! ## Does not own
//! - The profile object shape or its validation (see `retention`, `retention_mission`)
//! - Any tool-specific schema packaging (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to the published schema surface should normally require changes here.
//! Changes to mapping numbers should not require changes here unless the schema
//! documents them.
//!
//! Keep this module focused on profile schema publication.

use crate::admission_knobs::KNOWN_ADMISSION_KEYS;
use crate::retention::{
    BALANCED_READ_SIMILARITY, DEFAULT_RETENTION_FLOOR, INERT_FIELDS_HONORED_BY,
    LENIENT_READ_SIMILARITY, MAX_THETA_OFFSET, PERMISSIVE_THETA_DELTA, RETENTION_PROFILE_VERSION,
    SELECTIVE_NOVELTY_FLOOR, SELECTIVE_THETA_DELTA, STRICT_READ_SIMILARITY,
    STRICT_WRITE_NOVELTY_FLOOR,
};
use crate::retention_mission::{
    MAX_MISSION_ENTRY_CHARS, MAX_MISSION_EXAMPLES_PER_LIST, MAX_MISSION_STATEMENT_CHARS,
};
use serde_json::{Value, json};

/// One mission example entry schema, inlined into the keep/drop item schemas.
/// Kept as a definition-free inline object so the schema stays valid under any
/// document embedding (JSON Pointer `$ref`s are document-root-relative and
/// break when the schema is embedded elsewhere, e.g. in MCP tool input schemas).
fn mission_entry_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["text"],
        "properties": {
            "text": {
                "type": "string",
                "maxLength": MAX_MISSION_ENTRY_CHARS,
                "description": "A literal substring, or a pinned-subset regex when match is regex."
            },
            "match": {
                "type": "string",
                "enum": ["literal", "regex"],
                "default": "literal",
                "description": "Match discriminator; literal is the case-insensitive substring default."
            },
            "reject": {
                "type": "boolean",
                "default": true,
                "description": "Drop entries only: a hit rejects the write outright. Keep entries ignore this."
            }
        }
    })
}

/// Published JSON Schema for the profile object, with the exact mapping tables.
#[must_use]
pub fn profile_json_schema() -> Value {
    let key_names: Vec<&str> = KNOWN_ADMISSION_KEYS.to_vec();
    let mission_entry = mission_entry_schema();
    json!({
        "type": "object",
        "description": "Per-bank retention profile: verbosity, bounded per-category theta offsets, duplicate tolerance, recall scope, and an optional deterministic retention mission.",
        "additionalProperties": false,
        "x-retention-profile-version": RETENTION_PROFILE_VERSION,
        "x-verbosity-mapping": {
            "selective": {"theta_delta": SELECTIVE_THETA_DELTA, "novelty_floor": SELECTIVE_NOVELTY_FLOOR},
            "balanced": {"theta_delta": 0.0, "novelty_floor": 0.0},
            "permissive": {"theta_delta": PERMISSIVE_THETA_DELTA, "novelty_floor": 0.0}
        },
        "x-duplicate-tolerance-write-mapping": {
            "strict": {"novelty_floor": STRICT_WRITE_NOVELTY_FLOOR},
            "balanced": {"novelty_floor": 0.0},
            "lenient": {"novelty_floor": 0.0}
        },
        "x-duplicate-tolerance-read-mapping": {
            "strict": {"similarity_cap": STRICT_READ_SIMILARITY},
            "balanced": {"similarity_cap": BALANCED_READ_SIMILARITY},
            "lenient": {"similarity_cap": LENIENT_READ_SIMILARITY},
            "applies_when": "a scope preset or prefer_consolidated is active, or the level is strict/lenient; the neutral balanced default applies no cap"
        },
        "x-theta-offset-bound": MAX_THETA_OFFSET,
        "x-inert-fields-honored-by": INERT_FIELDS_HONORED_BY,
        "x-mission-mapping": {
            "drop_hit": {"utility_delta": -0.30, "rule": "drop wins over keep; reject=true entries set a mission_drop:<rule_id> rejection"},
            "keep_hit": {"utility_delta": 0.15, "rule": "ignored when a drop entry also hits; never rescues a gate failure"},
            "rule_id": "hex FNV-1a 64 of lowercase NFC-normalized entry text, a \\u{1} separator, and the match discriminator; stable across edits that do not touch the entry",
            "regex_subset": "literals, character classes, +/*/? quantifiers, | alternation only; no groups, backreferences, lookaround, or atomic groups",
            "match_semantics": "literal entries match case-insensitively as substrings; regex entries match case-insensitively over the pinned subset",
            "eval_bound": "regex evaluation is step-budgeted; entries over the budget are treated as non-matches and logged once"
        },
        "properties": {
            "verbosity": {
                "type": "string",
                "enum": ["selective", "balanced", "permissive"],
                "description": "Admission strictness. selective raises theta by 0.08 and sets a 0.15 novelty floor; balanced is the unchanged default; permissive lowers theta by 0.08."
            },
            "theta_offsets": {
                "type": "object",
                "description": "Bounded per-category delta added on top of the verbosity delta (abs(delta) <= 0.20).",
                "propertyNames": {"enum": key_names},
                "additionalProperties": {"type": "number", "minimum": -MAX_THETA_OFFSET, "maximum": MAX_THETA_OFFSET}
            },
            "duplicate_tolerance_write": {
                "type": "string",
                "enum": ["strict", "balanced", "lenient"],
                "description": "Admission-time novelty cutoff. strict rejects token-set similarity >= 0.75; balanced and lenient add no extra cutoff."
            },
            "duplicate_tolerance_read": {
                "type": "string",
                "enum": ["strict", "balanced", "lenient"],
                "description": "Read-time page similarity cap. Strict caps token-set Jaccard at 0.80, balanced at 0.90, lenient at 0.95; the neutral balanced default caps nothing unless a scope preset or prefer_consolidated is active."
            },
            "recall_scope_default": {
                "type": "string",
                "enum": ["full", "consolidated_only"],
                "description": "Default recall scope honored by retrieve/compose when the call omits recall_scope. consolidated_only returns eligible consolidated units only."
            },
            "retention_curve": {
                "type": "boolean",
                "default": true,
                "description": "Whether the retention curve shapes ranked results: each candidate's shaped score is multiplied by its age factor, never below retention_floor. On by default."
            },
            "retention_floor": {
                "type": "number",
                "exclusiveMinimum": 0.0,
                "maximum": 1.0,
                "default": DEFAULT_RETENTION_FLOOR,
                "description": "Lowest multiplier the retention curve may apply, in (0, 1]. Must be finite; 0 is refused, 1 is accepted."
            },
            "pack_fidelity_default": {
                "type": "string",
                "enum": ["flow", "exact"],
                "default": "flow",
                "description": "Default pack fidelity channel: flow (the default) or exact."
            },
            "provenance_floor_default": {
                "type": ["string", "null"],
                "enum": ["verified", "stated", "third_party", "inferred", null],
                "default": null,
                "description": "Optional floor on the provenance rung admitted when a caller sets none. Null means no floor; an unknown rung is refused."
            },
            "maintenance_trigger": {
                "type": "boolean",
                "default": true,
                "description": "On/off switch for the maintenance trigger. On by default."
            },
            "mission": {
                "type": "object",
                "description": "Optional deterministic retention mission: keep/drop examples that steer admission scoring. Admission influence is arithmetic only; mission text is never passed to any model.",
                "additionalProperties": false,
                "required": ["mission_statement", "drop_examples"],
                "properties": {
                    "mission_statement": {
                        "type": "string",
                        "maxLength": MAX_MISSION_STATEMENT_CHARS,
                        "description": "Human-readable mission statement (what to keep, what to drop, and the tie-break)."
                    },
                    "keep_examples": {
                        "type": "array",
                        "maxItems": MAX_MISSION_EXAMPLES_PER_LIST,
                        "items": mission_entry,
                        "description": "Patterns whose hits add +0.15 to the utility factor; never rescues a gate or span-verification failure."
                    },
                    "drop_examples": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_MISSION_EXAMPLES_PER_LIST,
                        "items": mission_entry,
                        "description": "Patterns whose hits subtract 0.30 from the utility factor; reject=true entries (the default) reject outright with mission_drop:<rule_id>."
                    }
                }
            },
            "version": {
                "type": "integer",
                "const": RETENTION_PROFILE_VERSION,
                "description": "Profile schema version stamp."
            }
        },
        "required": []
    })
}

/// True when every `$ref` anywhere in `schema` resolves to an existing
/// location in the same document (RFC 6901 JSON Pointer, with `~0`/`~1`
/// escapes). Schema-pack tests run this over every published schema so a
/// dangling pointer can never ship again.
#[must_use]
pub fn schema_refs_resolve(schema: &Value) -> bool {
    fn resolve_pointer<'a>(root: &'a Value, pointer: &str) -> Option<&'a Value> {
        if pointer.is_empty() || pointer == "#" {
            return Some(root);
        }
        let path = pointer.strip_prefix('#').unwrap_or(pointer);
        let path = path.strip_prefix('/')?;
        let mut node = root;
        for raw in path.split('/') {
            let token = raw.replace("~1", "/").replace("~0", "~");
            node = match node {
                Value::Object(map) => map.get(&token)?,
                Value::Array(items) => items.get(token.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(node)
    }
    let mut stack = vec![schema];
    while let Some(node) = stack.pop() {
        match node {
            Value::Object(map) => {
                if let Some(Value::String(reference)) = map.get("$ref")
                    && resolve_pointer(schema, reference).is_none()
                {
                    return false;
                }
                stack.extend(map.values());
            }
            Value::Array(items) => stack.extend(items.iter()),
            _ => {}
        }
    }
    true
}

#[cfg(test)]
#[path = "retention_schema_tests.rs"]
mod retention_schema_tests;
