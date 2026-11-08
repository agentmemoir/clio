// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for assembling, stamping, and validating the
//! machine-readable `tool_schema` pack for the Core catalog.
//!
//! ## Owns
//! - Pack document shape (`format`, revision stamp, catalog hash, tools)
//! - The shared FNV-1a 64-bit hex hash convention
//! - Catalog-hash drift detection and structural schema validation
//!
//! ## Does not own
//! - Per-tool parameter definitions (see `schema_defs`)
//! - Transport registration or dispatch
//!
//! ## Boundary
//!
//! Changes to pack packaging or validation should normally require changes here.
//! Changes to a tool's parameters should not require changes here.
//!
//! Keep this module focused on the schema pack.

use crate::MCP_PROTOCOL_REVISION;
use crate::schema_additive_defs::additive_defs;
use crate::schema_config_defs::config_defs;
use crate::schema_defs::{ToolDef, core_defs};
use crate::schema_hygiene_defs::hygiene_defs;
use crate::schema_ops_defs::ops_defs;
use crate::schema_portability_defs::portability_defs;
use crate::schema_request_defs::request_defs;
use crate::schema_retention_defs::retention_defs;
use crate::schema_sync_defs::sync_defs;
use serde_json::{Value, json};

/// Schema pack format tag (never conflated with semantic category `schema`).
pub const PACK_FORMAT: &str = "am_tool_schema_pack";

/// Every published tool definition: the Core catalog plus the additive
/// harness workspace tools, the hygiene tools, the
/// multi-host sync tools, the retention-profile tools, and the
/// config/ranking tools.
#[must_use]
pub fn catalog_defs() -> Vec<ToolDef> {
    let mut all = core_defs();
    all.extend(additive_defs());
    all.extend(hygiene_defs());
    all.extend(portability_defs());
    all.extend(request_defs());
    all.extend(ops_defs());
    all.extend(sync_defs());
    all.extend(retention_defs());
    all.extend(config_defs());
    all
}

/// One definition from the whole catalog, by exact name.
///
/// The lookup reads every definition this build publishes, so neither the tool
/// profile the configuration selects nor the bound set can narrow it: a tool a
/// listing hides still resolves to the definition it is dispatched with.
#[must_use]
pub fn catalog_def(name: &str) -> Option<ToolDef> {
    catalog_defs().into_iter().find(|def| def.name == name)
}

/// Build the complete `tool_schema` pack for the bound catalog.
///
/// The pack carries the pinned MCP revision, a deterministic catalog hash,
/// and the published definition of that hash, so drift between the schema pack
/// and the registered tool set is detectable and the digest is recomputable.
#[must_use]
pub fn schema_pack() -> Value {
    let tools: Vec<Value> = catalog_defs().iter().map(ToolDef::to_tool).collect();
    pack_of(&tools)
}

/// Build the `tool_schema` pack holding only the tools `groups` select.
///
/// The selection resolves through [`clio_types::tools_of`], the same derivation
/// a preset resolves through, so a narrowed pack and a listing made under that
/// selection hold the same tools. The hash covers the tools the pack publishes,
/// exactly as it does for the full pack.
#[must_use]
pub fn schema_pack_for_groups(groups: &[clio_types::ToolGroup]) -> Value {
    let names = clio_types::tools_of(groups);
    let tools: Vec<Value> = catalog_defs()
        .iter()
        .filter(|def| names.contains(&def.name))
        .map(ToolDef::to_tool)
        .collect();
    pack_of(&tools)
}

/// Assemble the pack document around `tools`, so the object shape is written
/// once and a narrowed pack cannot drift from the full one.
fn pack_of(tools: &[Value]) -> Value {
    json!({
        "format": PACK_FORMAT,
        "format_version": 1,
        "mcp_protocol_revision": MCP_PROTOCOL_REVISION,
        "catalog_hash": catalog_hash(tools),
        "catalog_hash_definition": catalog_hash_definition(),
        "tools": tools
    })
}

/// Serialize a pack with one top-level key per line and one array element per
/// line, so a committed pack stays small and each tool change diffs on one
/// line. The output parses to the same value as the compact encoding.
#[must_use]
pub fn to_export_json(pack: &Value) -> String {
    let mut out = String::from("{\n");
    if let Some(fields) = pack.as_object() {
        let last = fields.len().saturating_sub(1);
        for (index, (key, value)) in fields.iter().enumerate() {
            out.push_str("  ");
            out.push_str(&serde_json::to_string(key).unwrap_or_default());
            out.push_str(": ");
            if let Some(items) = value.as_array() {
                out.push_str("[\n");
                for (item_index, item) in items.iter().enumerate() {
                    out.push_str("    ");
                    out.push_str(&serde_json::to_string(item).unwrap_or_default());
                    if item_index + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str("  ]");
            } else {
                out.push_str(&serde_json::to_string(value).unwrap_or_default());
            }
            if index < last {
                out.push(',');
            }
            out.push('\n');
        }
    }
    out.push_str("}\n");
    out
}

/// FNV-1a 64-bit hash over `data`, formatted as `fnv1a64:` plus 16 lowercase
/// hex digits. This is the shared hash convention for canonical payload
/// fingerprints; each hashed payload defines its own canonical bytes, so a
/// digest is meaningful only for the payload it covers.
#[must_use]
pub fn fnv1a64_hex(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

/// The published definition that accompanies the hash: the algorithm, its
/// version tag, the two constants, the exact input it covers, the order the
/// input is concatenated in, and the rendered form. It ships beside the hash
/// it describes in every pack, so a reader can recompute the value without
/// reading this implementation.
#[must_use]
fn catalog_hash_definition() -> Value {
    json!({
        "algorithm": "FNV-1a",
        "version": "fnv1a64",
        "hash_bits": 64,
        "offset_basis": "0xcbf29ce484222325",
        "prime": "0x100000001b3",
        "input": "compact UTF-8 JSON encoding of every tool definition published in tools[], including name, description, inputSchema and outputSchema, with object keys in lexicographic order and array order preserved",
        "order": "catalog registry order",
        "concatenation": "adjacent bytes, no separator",
        "rendered_form": "fnv1a64:{hash:016x}",
        "rendered_form_is_external_contract": true
    })
}

/// FNV-1a hash over the canonical serialization of the tool list: the
/// compact encodings of every tool, concatenated in registry order.
#[must_use]
pub fn catalog_hash(tools: &[Value]) -> String {
    let mut canonical = Vec::new();
    for tool in tools {
        let encoded = serde_json::to_string(tool).unwrap_or_default();
        canonical.extend_from_slice(encoded.as_bytes());
    }
    fnv1a64_hex(&canonical)
}

/// Structurally validate one tool schema: root object type, properties
/// map, required list. Returns a human-readable defect list.
#[must_use]
pub fn validate_tool_schema(tool: &Value) -> Vec<String> {
    let mut defects = Vec::new();
    let name = tool.get("name").and_then(Value::as_str).unwrap_or("?");
    let Some(schema) = tool.get("inputSchema") else {
        defects.push(format!("tool `{name}` missing inputSchema"));
        return defects;
    };
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        defects.push(format!("`{name}`: inputSchema root type must be object"));
    }
    let props = schema.get("properties").and_then(Value::as_object);
    if props.is_none() {
        defects.push(format!(
            "`{name}`: inputSchema.properties must be an object"
        ));
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for req in required {
            let prop = req.as_str().unwrap_or_default();
            if !props.is_some_and(|p| p.contains_key(prop)) {
                defects.push(format!(
                    "`{name}`: required property `{prop}` has no schema entry"
                ));
            }
        }
    }
    if !clio_config::retention_schema::schema_refs_resolve(schema) {
        defects.push(format!(
            "`{name}`: inputSchema contains an unresolvable $ref JSON Pointer"
        ));
    }
    defects
}

#[cfg(test)]
#[path = "catalog_lookup_tests.rs"]
mod catalog_lookup_tests;

#[cfg(test)]
#[path = "schema_tests.rs"]
mod schema_tests;

#[cfg(test)]
#[path = "schema_context_read_schema_tests.rs"]
mod schema_context_read_schema_tests;

#[cfg(test)]
#[path = "schema_hash_tests.rs"]
mod schema_hash_tests;

#[cfg(test)]
#[path = "schema_local_defs_tests.rs"]
mod schema_local_defs_tests;
