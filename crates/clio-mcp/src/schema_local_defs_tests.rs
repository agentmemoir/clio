// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin every emitted tool schema
//! to a self-contained document: a local reference resolves inside the tool's
//! own root `$defs`, and no part of the document needs a network fetch.
//!
//! ## Owns
//! - Metaschema validity and offline compilation for all three emission paths
//! - A dangling local reference failing both compilation and pack validation
//! - Reference emission being smaller than its flattened expansion where the
//!   mission entry is defined once and referenced twice, plus the batch
//!   candidate definition referenced once
//! - The retention tools accepting and rejecting arguments their own published
//!   schema constrains
//! - Each retention tool resolving to the shape the base commit published
//! - The reserved `$defs` property channel never leaking into `properties`
//!
//! ## Does not own
//! - The pack shape or its hash (`schema_tests`, `schema_hash_tests`)
//! - Per-tool parameter definitions (`schema_defs`, `schema_retention_defs`)
//!
//! ## Boundary
//!
//! Changes to how a tool schema is emitted should normally require changes
//! here. Changes to what a tool declares should not.
//!
//! Keep this module focused on emitted-schema self-containment.

use super::*;
use crate::protocol::McpHandler;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// The three ways this crate hands a tool document to a caller.
const EMISSION_PATHS: [&str; 3] = ["pack", "catalog", "listing"];

/// The retention tools whose profile properties carry the shared mission entry.
const MISSION_TOOLS: [&str; 2] = ["admit_preview_batch", "retention_profile_set"];

/// A server state whose configuration layer is exactly `config`.
///
/// The runtime reads no process environment, so the listing depends on the
/// given document alone.
fn state_with(config: &str) -> Arc<McpState> {
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    runtime
        .load_file_json(config)
        .expect("a valid config layer");
    McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state")
}

fn tool_name(tool: &Value) -> String {
    tool["name"].as_str().expect("a tool name").to_owned()
}

/// Every tool document this build emits, with the path that emitted it.
fn emitted_tools() -> Vec<(&'static str, String, Value)> {
    let mut emitted = Vec::new();
    for tool in schema_pack()["tools"].as_array().expect("tools") {
        emitted.push(("pack", tool_name(tool), tool.clone()));
    }
    for name in catalog_defs().iter().map(|def| def.name) {
        let def = catalog_def(name).expect("every catalog name resolves");
        emitted.push(("catalog", name.to_owned(), def.to_tool()));
    }
    let state = state_with(r#"{"mcp": {"tool_profile": "all"}}"#);
    let listing = McpHandler::tools_list_visible(&state).expect("a known profile lists");
    for tool in listing["tools"].as_array().expect("tools") {
        emitted.push(("listing", tool_name(tool), tool.clone()));
    }
    emitted
}

#[test]
fn every_emitted_schema_validates_alone_with_network_denied() {
    let emitted = emitted_tools();
    // One build per distinct schema: a build is slow, and identical bytes from
    // two paths prove the same thing twice.
    let mut by_schema: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for (path, name, tool) in &emitted {
        for key in ["inputSchema", "outputSchema"] {
            if let Some(schema) = tool.get(key) {
                let encoded = serde_json::to_string(schema).expect("a schema");
                by_schema
                    .entry(encoded)
                    .or_default()
                    .push(format!("{path}:{name}.{key}"));
            }
        }
    }
    assert!(!by_schema.is_empty(), "the catalog emitted no schema");
    for (encoded, sources) in &by_schema {
        let schema: Value = serde_json::from_str(encoded).expect("a schema");
        assert!(
            jsonschema::draft202012::meta::is_valid(&schema),
            "{sources:?} is not a valid schema document"
        );
        jsonschema::options()
            .offline()
            .build(&schema)
            .unwrap_or_else(|error| panic!("{sources:?} needs a fetch: {error}"));
    }
    // Every path emits, and the three paths agree on the tool name set.
    let names_of = |path: &str| {
        let mut names: Vec<String> = emitted
            .iter()
            .filter(|(emitted_by, _, _)| *emitted_by == path)
            .map(|(_, name, _)| name.clone())
            .collect();
        names.sort_unstable();
        names
    };
    let pack = names_of(EMISSION_PATHS[0]);
    assert!(!pack.is_empty(), "the pack emits no tool");
    for path in EMISSION_PATHS {
        assert_eq!(pack, names_of(path), "{path} disagrees on the tool set");
    }
}

/// The value one local `#/$defs/...` reference points at, resolved inside
/// `root`.
fn pointed<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let mut node = root;
    for token in reference.strip_prefix("#/")?.split('/') {
        node = node.get(token)?;
    }
    Some(node)
}

/// `node` with every local reference replaced by the definition it points at,
/// recursively, so the result holds no `$ref`.
fn flattened(node: &Value, root: &Value) -> Value {
    match node {
        Value::Object(map) => {
            if let Some(target) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| pointed(root, reference))
            {
                return flattened(target, root);
            }
            Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), flattened(value, root)))
                    .collect(),
            )
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| flattened(item, root)).collect())
        }
        other => other.clone(),
    }
}

/// A schema with its root `$defs` removed: the document a flattener produces,
/// and the document a resolver would have to fetch into.
fn without_root_defs(schema: &Value) -> Value {
    let mut copy = schema.clone();
    if let Some(map) = copy.as_object_mut() {
        map.remove("$defs");
    }
    copy
}

/// How many string `$ref` values anywhere in `value` equal `reference`.
fn count_refs(value: &Value, reference: &str) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(map.get("$ref").and_then(Value::as_str) == Some(reference))
                + map
                    .values()
                    .map(|child| count_refs(child, reference))
                    .sum::<usize>()
        }
        Value::Array(items) => items.iter().map(|item| count_refs(item, reference)).sum(),
        _ => 0,
    }
}

fn retention_input(name: &str) -> Value {
    catalog_def(name).expect("a retention tool").to_tool()["inputSchema"].clone()
}

/// The resolved input schemas of both retention tools: each tool's document
/// with every local reference flattened and its root `$defs` removed. The guard
/// below compares that resolved contract, so a default, enum, or description
/// change fails it while a change in reference packing does not.
///
/// The recorded batch element gained `type` and the single-item `required` list
/// when the element became a local reference to the shared admission candidate
/// definition, so this file pins the shape that reference resolves to.
const RETENTION_BASE_INPUT_SCHEMAS: &str =
    include_str!("../tests/fixtures/retention_base_input_schemas.json");

#[test]
fn retention_input_schemas_resolve_to_the_pre_change_base_shape() {
    let base: Value =
        serde_json::from_str(RETENTION_BASE_INPUT_SCHEMAS).expect("the base fixture is JSON");
    for name in MISSION_TOOLS {
        let live = retention_input(name);
        let resolved = without_root_defs(&flattened(&live, &live));
        let expected = &base[name];
        assert_eq!(
            &resolved, expected,
            "{name}: the resolved contract drifted from the base commit"
        );
        assert_eq!(
            serde_json::to_string(&resolved)
                .expect("resolved bytes")
                .len(),
            serde_json::to_string(expected).expect("base bytes").len(),
            "{name}: the resolved contract changed size"
        );
    }
}

#[test]
fn no_emitted_tool_exposes_the_reserved_defs_channel_as_a_property() {
    for (path, name, tool) in emitted_tools() {
        let properties = tool["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{path}:{name} declares no properties"));
        assert!(
            !properties.contains_key("$defs"),
            "{path}:{name} leaks the reserved `$defs` channel into properties"
        );
    }
}

#[test]
fn a_reference_without_local_definitions_fails() {
    let tool = catalog_def("retention_profile_set")
        .expect("the retention tool")
        .to_tool();
    let schema = &tool["inputSchema"];
    assert!(
        schema.get("$defs").is_some(),
        "the published schema carries the definition its reference needs"
    );
    assert!(
        count_refs(schema, "#/$defs/MissionEntry") > 0,
        "the published schema holds a local reference"
    );
    let dangling = without_root_defs(schema);
    jsonschema::options()
        .offline()
        .build(&dangling)
        .expect_err("a dangling local reference must not compile");
    let mut published = tool.clone();
    published["inputSchema"] = dangling;
    assert!(
        !validate_tool_schema(&published).is_empty(),
        "pack validation must reject a dangling local reference"
    );
}

#[test]
fn reference_based_emission_is_smaller_than_the_flattened_candidate() {
    // `retention_profile_set` references the shared mission entry twice, so one
    // definition plus two pointers is smaller than two inline copies.
    let schema = retention_input("retention_profile_set");
    let emitted = serde_json::to_string(&schema).expect("emitted");
    let expanded =
        serde_json::to_string(&without_root_defs(&flattened(&schema, &schema))).expect("flattened");
    assert!(
        expanded.len() > emitted.len(),
        "retention_profile_set: emitted {} bytes, flattened {} bytes",
        emitted.len(),
        expanded.len()
    );
    let defs = schema["$defs"].as_object().expect("root $defs");
    assert_eq!(
        defs.len(),
        1,
        "retention_profile_set: one shared definition"
    );
    assert!(defs.contains_key("MissionEntry"), "the shared one");
    assert_eq!(
        count_refs(&schema, "#/$defs/MissionEntry"),
        2,
        "both mission lists reference the shared definition"
    );
    println!(
        "retention_profile_set: emitted {} bytes, flattened {} bytes",
        emitted.len(),
        expanded.len()
    );

    // `admit_preview_batch` references its candidate definition once behind a
    // `$defs` wrapper, which is larger than the inline copy, so its guard is
    // the reference and definition count rather than the byte size.
    let batch = retention_input("admit_preview_batch");
    let candidate_ref = "#/$defs/AdmissionCandidate";
    assert_eq!(
        count_refs(&batch, candidate_ref),
        1,
        "admit_preview_batch: one candidate reference"
    );
    let batch_defs = batch["$defs"].as_object().expect("root $defs");
    assert_eq!(
        batch_defs.len(),
        2,
        "admit_preview_batch: the candidate plus the mission entry"
    );
    assert!(batch_defs.contains_key("AdmissionCandidate"));
    assert!(batch_defs.contains_key("MissionEntry"));

    let pack = schema_pack();
    let published = pack["tools"]
        .as_array()
        .expect("the pack holds tools")
        .iter()
        .find(|tool| tool["name"] == json!("admit_preview_batch"))
        .expect("the pack publishes the batch preview");
    assert_eq!(count_refs(published, candidate_ref), 1);
    assert_eq!(
        published["inputSchema"]["$defs"]
            .as_object()
            .expect("root $defs")
            .len(),
        2
    );
}

/// A minimal instance of the argument type `schema` declares, for the object
/// and array arguments the retention tools require.
fn minimal_value(schema: &Value) -> Value {
    match schema["type"].as_str() {
        Some("object") => Value::Object(Map::new()),
        Some("array") => Value::Array(Vec::new()),
        other => panic!("no minimal instance for type {other:?}"),
    }
}

/// The first `(outer property, inner property, allowed values)` where an
/// object-valued argument declares an enum one level down.
fn nested_enum(schema: &Value) -> Option<(String, String, Vec<String>)> {
    for (outer, outer_schema) in schema["properties"].as_object()? {
        let Some(inner) = outer_schema["properties"].as_object() else {
            continue;
        };
        for (inner_name, inner_schema) in inner {
            let Some(values) = inner_schema["enum"].as_array() else {
                continue;
            };
            let allowed: Vec<String> = values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            if !allowed.is_empty() {
                return Some((outer.clone(), inner_name.clone(), allowed));
            }
        }
    }
    None
}

#[test]
fn retention_schemas_accept_and_reject_the_same_arguments() {
    for name in MISSION_TOOLS {
        let schema = retention_input(name);
        let validator = jsonschema::options()
            .offline()
            .build(&schema)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let required: Vec<String> = schema["required"]
            .as_array()
            .expect("a required list")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let first = required.first().expect("a required argument").clone();

        let mut accepted = Map::new();
        for property in &required {
            accepted.insert(
                property.clone(),
                minimal_value(&schema["properties"][property]),
            );
        }
        let accepted_case = Value::Object(accepted.clone());
        assert!(
            validator.is_valid(&accepted_case),
            "{name} rejects a minimal valid call: {accepted_case}"
        );

        let mut missing = accepted.clone();
        missing.remove(&first);
        assert!(
            !validator.is_valid(&Value::Object(missing)),
            "{name} accepts a call missing `{first}`"
        );

        let mut mistyped = accepted.clone();
        mistyped.insert(first.clone(), json!("not the declared type"));
        assert!(
            !validator.is_valid(&Value::Object(mistyped)),
            "{name} accepts a string where `{first}` declares {}",
            schema["properties"][&first]["type"]
        );

        let (outer, inner, allowed) =
            nested_enum(&schema).unwrap_or_else(|| panic!("{name} declares no enum to test"));
        let mut known_inner = Map::new();
        known_inner.insert(inner.clone(), json!(allowed[0]));
        let mut known = accepted.clone();
        known.insert(outer.clone(), Value::Object(known_inner));
        assert!(
            validator.is_valid(&Value::Object(known)),
            "{name} rejects the first allowed `{outer}.{inner}` value"
        );
        let mut unknown_inner = Map::new();
        unknown_inner.insert(inner.clone(), json!("no-such-member"));
        let mut unknown = accepted;
        unknown.insert(outer.clone(), Value::Object(unknown_inner));
        assert!(
            !validator.is_valid(&Value::Object(unknown)),
            "{name} accepts `{outer}.{inner}` outside its enum"
        );
    }
}
