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

/// Build the complete `tool_schema` pack for the bound catalog.
///
/// The pack carries the pinned MCP revision and a deterministic catalog hash
/// so drift between the schema pack and the registered tool set is detectable.
#[must_use]
pub fn schema_pack() -> Value {
    let tools: Vec<Value> = catalog_defs().iter().map(ToolDef::to_tool).collect();
    json!({
        "format": PACK_FORMAT,
        "format_version": 1,
        "mcp_protocol_revision": MCP_PROTOCOL_REVISION,
        "catalog_hash": catalog_hash(&tools),
        "tools": tools
    })
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

/// FNV-1a hash over the canonical serialization of the tool list: the
/// compact encodings of every tool, concatenated in registry order.
#[must_use]
fn catalog_hash(tools: &[Value]) -> String {
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
#[path = "schema_tests.rs"]
mod schema_tests;

#[cfg(test)]
#[path = "schema_context_read_schema_tests.rs"]
mod schema_context_read_schema_tests;
