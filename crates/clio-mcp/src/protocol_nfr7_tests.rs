// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the schema-driven exercisability test:
//! every published tool is exercised through its own published JSON schema over
//! the shared MCP dispatcher, without depending on any implementation-language
//! surface.
//!
//! ## Owns
//! - Minimal-argument derivation from a published `inputSchema`
//! - The recorded provider-exception list and its fail-closed assertions
//!
//! ## Does not own
//! - Tool dispatch semantics (see `write_tools` / the tool modules)
//! - Schema definitions (see `schema_defs` and friends)
//!
//! ## Boundary
//!
//! Changes to what "exercisable through the schema" means should normally
//! require changes here. Changes to individual tool behavior should not.
//!
//! Keep this module focused on the schema-driven exercisability contract.

use crate::runtime::{McpOpenOptions, McpState};
use crate::schema::schema_pack;
use crate::write_tools::dispatch;
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// Tools that cannot be dispatched without a live external provider.
///
/// Each entry carries the recorded exception reason: the test still asserts
/// schema validity and bound-name equality for these, plus their documented
/// fail-closed dispatch behavior. `summarize` needs a configured extraction
/// provider; without one it must fail closed (the extraction-unavailable error)
/// rather than fabricate gist text.
const PROVIDER_EXCEPTIONS: &[(&str, &str)] = &[(
    "summarize",
    "regenerating gists requires a configured extraction provider; without one the tool fails closed with a structured error and fabricates no text",
)];

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

/// Derive a minimal valid argument instance for one property schema node.
fn minimal_instance(schema: &Value, root: &Value) -> Value {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return values.first().cloned().unwrap_or(Value::Null);
    }
    if let Some(pointer) = schema.get("$ref").and_then(Value::as_str) {
        return minimal_instance(resolve_ref(root, pointer), root);
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut out = Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for name in schema
                    .get("required")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(name) = name.as_str() else { continue };
                    let Some(prop) = props.get(name) else {
                        continue;
                    };
                    out.insert(name.to_owned(), minimal_instance(prop, root));
                }
            }
            Value::Object(out)
        }
        Some("array") => Value::Array(vec![]),
        Some("boolean") => Value::Bool(false),
        Some("number" | "integer") => json!(0),
        Some("string") => json!(""),
        _ => Value::Null,
    }
}

fn resolve_ref<'a>(root: &'a Value, pointer: &str) -> &'a Value {
    let body = pointer.strip_prefix("#").unwrap_or(pointer);
    if body.is_empty() {
        return root;
    }
    let mut node = root;
    for segment in body.trim_start_matches('/').split('/') {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        node = node.get(&segment).unwrap_or(&Value::Null);
    }
    node
}

/// Build minimal arguments from one published tool's `inputSchema`.
fn minimal_args(tool: &Value) -> Value {
    let schema = &tool["inputSchema"];
    minimal_instance(schema, schema)
}

/// Dispatch every published tool through its schema.
///
/// For each tool: the derived minimal arguments produce a structured MCP
/// response (never a panic), and a failure must carry a typed code other than
/// the `not_implemented` unknown-tool fallback. Tools needing a live provider
/// are asserted against their recorded exception instead.
#[test]
fn nfr7_every_published_tool_dispatches_through_its_schema() {
    let s = state();
    let pack = schema_pack();
    let tools = pack["tools"].as_array().expect("tools array");
    let mut exercised = 0_usize;
    for tool in tools {
        let name = tool["name"].as_str().expect("tool name");
        let args = minimal_args(tool);
        let result = dispatch(&s, name, &args);
        assert!(
            result.is_object(),
            "{name}: dispatch must return a structured object, got {result}"
        );
        if PROVIDER_EXCEPTIONS.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let ok = result.get("ok").and_then(Value::as_bool) == Some(true);
        if !ok {
            let code = result.get("code").and_then(Value::as_str).unwrap_or("?");
            assert_ne!(
                code, "not_implemented",
                "bound tool `{name}` fell through to not_implemented: {result}"
            );
        }
        exercised += 1;
    }
    assert!(exercised > 0, "the pack must exercise most tools");
}

/// For each recorded provider exception: the tool stays published and bound,
/// and its dispatch fails closed with its documented reason.
#[test]
fn nfr7_recorded_exceptions_fail_closed_with_reason() {
    let s = state();
    let pack = schema_pack();
    for (name, _reason) in PROVIDER_EXCEPTIONS {
        let tool = pack["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|t| t["name"].as_str() == Some(*name))
            .unwrap_or_else(|| panic!("{name} missing from the published pack"));
        assert!(
            crate::bound_tools().contains(name),
            "{name} must stay bound"
        );
        assert!(
            !tool["inputSchema"].is_null(),
            "{name} must keep a published schema"
        );
        let result = dispatch(&s, name, &minimal_args(tool));
        assert_eq!(
            result.get("ok").and_then(Value::as_bool),
            Some(false),
            "{name}: {result}"
        );
        // The documented fail-closed marker. The recorded reason above states
        // why the tool is exempted from the dispatch assertion; the response
        // must carry the extraction-unavailable error, never fabricated text.
        assert!(
            result["message"]
                .as_str()
                .unwrap_or_default()
                .contains("extraction unavailable"),
            "{name} must fail closed with the extraction-unavailable error: {result}"
        );
    }
}
