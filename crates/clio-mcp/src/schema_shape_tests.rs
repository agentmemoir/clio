// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin each pack shape to what it
//! promises: the reference shape keeps its local references and definitions,
//! and the flattened shape inlines them so the document holds neither.
//!
//! ## Owns
//! - Flattening nested local references leaving zero `$ref` and zero `$defs`
//! - Flattening preserving the content every reference resolved to
//! - Every flattened schema still validating and compiling alone
//! - The reference shape keeping the references and definitions it ships
//!
//! ## Does not own
//! - Which shape a pack is built in, or the pack document (`schema_tests`,
//!   the pack snapshot test)
//! - Per-tool parameter definitions (`schema_defs`, `schema_retention_defs`)
//!
//! ## Boundary
//!
//! Changes to what a shape resolves to should normally require changes here.
//! Changes to what a tool declares should not require changes here.
//!
//! Keep this module focused on the two shapes' promises.

use super::{PackShape, flatten};
use serde_json::{Value, json};

/// A document whose references nest: a property points at a definition that
/// itself points at another definition, and one definition is referenced twice.
fn nested_reference_document() -> Value {
    json!({
        "$defs": {
            "Leaf": {"type": "string", "description": "a leaf"},
            "Entry": {
                "type": "object",
                "additionalProperties": false,
                "required": ["leaf"],
                "properties": {"leaf": {"$ref": "#/$defs/Leaf"}}
            }
        },
        "type": "object",
        "properties": {
            "first": {"$ref": "#/$defs/Entry"},
            "second": {"$ref": "#/$defs/Entry"},
            "third": {"$ref": "#/$defs/Entry/properties/leaf"},
            "plain": {"type": "number"}
        }
    })
}

/// The same document with every reference written out.
fn nested_inline_document() -> Value {
    let entry = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["leaf"],
        "properties": {"leaf": {"type": "string", "description": "a leaf"}}
    });
    json!({
        "type": "object",
        "properties": {
            "first": entry,
            "second": entry,
            "third": {"type": "string", "description": "a leaf"},
            "plain": {"type": "number"}
        }
    })
}

/// How many `keyword` keys appear anywhere in `value`.
fn count(value: &Value, keyword: &str) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(map.contains_key(keyword))
                + map
                    .values()
                    .map(|child| count(child, keyword))
                    .sum::<usize>()
        }
        Value::Array(items) => items.iter().map(|item| count(item, keyword)).sum(),
        _ => 0,
    }
}

#[test]
fn flattening_nested_local_references_leaves_no_reference_and_no_definitions() {
    let reference = nested_reference_document();
    assert_eq!(count(&reference, "$ref"), 4, "the reference document");
    assert!(count(&reference, "$defs") >= 1, "the reference document");
    let flattened = flatten(&reference);
    assert_eq!(
        count(&flattened, "$ref"),
        0,
        "no reference survives: {flattened}"
    );
    assert_eq!(
        count(&flattened, "$defs"),
        0,
        "no definitions survive: {flattened}"
    );
}

#[test]
fn flattening_preserves_the_content_every_reference_resolved_to() {
    let flattened = flatten(&nested_reference_document());
    assert_eq!(flattened, nested_inline_document());
}

#[test]
fn a_flattened_schema_still_validates_and_compiles_alone() {
    let flattened = flatten(&nested_reference_document());
    assert!(
        jsonschema::draft202012::meta::is_valid(&flattened),
        "{flattened}"
    );
    let validator = jsonschema::options()
        .offline()
        .build(&flattened)
        .expect("a flattened schema compiles with no fetch");
    let entry = json!({"leaf": "a leaf"});
    assert!(validator.is_valid(&json!({"first": entry, "second": entry})));
    assert!(!validator.is_valid(&json!({"first": {"leaf": 3}})));
}

#[test]
fn the_reference_shape_keeps_its_references_and_definitions() {
    let reference = nested_reference_document();
    let kept = PackShape::Reference.schema(&reference);
    assert_eq!(kept, reference);
    assert_eq!(count(&kept, "$ref"), 4);
    assert!(kept["$defs"].is_object(), "the definitions ship: {kept}");
}

#[test]
fn the_flattened_shape_holds_neither_reference_nor_definitions() {
    let flattened = PackShape::Flattened.schema(&nested_reference_document());
    assert_eq!(count(&flattened, "$ref"), 0);
    assert_eq!(count(&flattened, "$defs"), 0);
}

#[test]
fn a_tool_is_shaped_schema_by_schema() {
    let tool = json!({
        "name": "shaped",
        "description": "a tool whose input and output both reference a definition",
        "inputSchema": {
            "type": "object",
            "properties": {"a": {"$ref": "#/$defs/A"}},
            "$defs": {"A": {"type": "string"}}
        },
        "outputSchema": {
            "type": "object",
            "properties": {"b": {"$ref": "#/$defs/B"}},
            "$defs": {"B": {"type": "integer"}}
        }
    });
    let flattened = PackShape::Flattened.tool(&tool);
    assert_eq!(flattened["name"], tool["name"]);
    assert_eq!(flattened["description"], tool["description"]);
    assert_eq!(
        flattened["inputSchema"],
        json!({"type": "object", "properties": {"a": {"type": "string"}}})
    );
    assert_eq!(
        flattened["outputSchema"],
        json!({"type": "object", "properties": {"b": {"type": "integer"}}})
    );
    assert_eq!(count(&flattened, "$ref"), 0);
    assert_eq!(count(&flattened, "$defs"), 0);
    assert_eq!(PackShape::Reference.tool(&tool), tool);
}

#[test]
fn an_unresolvable_reference_is_left_exactly_as_written() {
    let document = json!({
        "type": "object",
        "properties": {
            "external": {"$ref": "https://example.invalid/schema.json"},
            "dangling": {"$ref": "#/$defs/Missing"}
        },
        "$defs": {"Unused": {"type": "string"}}
    });
    let flattened = flatten(&document);
    assert_eq!(
        flattened["properties"]["external"],
        json!({"$ref": "https://example.invalid/schema.json"})
    );
    assert_eq!(
        flattened["properties"]["dangling"],
        json!({"$ref": "#/$defs/Missing"})
    );
    assert_eq!(
        count(&flattened, "$defs"),
        0,
        "unused definitions are dropped"
    );
}
