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
//! - Recursive documents rewriting to a bounded, deterministic result
//! - A rewrite that keeps a reference keeping the definitions it resolves
//!   against, at every level
//! - A reference beside a sibling keyword staying exactly as written
//! - The local pointer walk: the document root, array indices, and members
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

use super::{PackShape, flatten, pointed};
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
            "dangling": {"$ref": "#/$defs/DoesNotExist"}
        },
        "$defs": {"Unused": {"type": "string"}}
    });
    let flattened = flatten(&document);
    assert_eq!(
        flattened["properties"]["dangling"],
        json!({"$ref": "#/$defs/DoesNotExist"})
    );
    assert_eq!(
        count(&flattened, "$defs"),
        0,
        "unused definitions are dropped"
    );
}

/// A document whose root refers to itself: each reference names the whole
/// document, directly or through the empty fragment.
fn self_referential_document() -> Value {
    json!({
        "type": "object",
        "properties": {
            "self": {"$ref": "#"},
            "whole": {"$ref": ""},
            "plain": {"type": "string"}
        }
    })
}

/// A document whose two definitions refer to one another, so neither can be
/// inlined on its own.
fn mutually_recursive_document() -> Value {
    json!({
        "$defs": {
            "Even": {"type": "object", "properties": {"next": {"$ref": "#/$defs/Odd"}}},
            "Odd": {"type": "object", "properties": {"next": {"$ref": "#/$defs/Even"}}}
        },
        "type": "object",
        "properties": {"start": {"$ref": "#/$defs/Even"}}
    })
}

/// A document holding a `$defs` inside a definition, referenced from the root.
fn nested_defs_document() -> Value {
    json!({
        "$defs": {
            "Envelope": {
                "$defs": {"Inner": {"type": "string", "description": "an inner leaf"}},
                "type": "object",
                "properties": {"leaf": {"$ref": "#/$defs/Envelope/$defs/Inner"}}
            }
        },
        "type": "object",
        "properties": {"envelope": {"$ref": "#/$defs/Envelope"}}
    })
}

/// A document whose reference carries a sibling keyword beside it.
fn sibling_reference_document() -> Value {
    json!({
        "$defs": {"Label": {"type": "string", "description": "a label"}},
        "type": "object",
        "properties": {"label": {"$ref": "#/$defs/Label", "maxLength": 8}}
    })
}

/// True when every `$ref` anywhere in `value` resolves inside `value` itself.
fn every_reference_resolves(value: &Value) -> bool {
    clio_config::retention_schema::schema_refs_resolve(value)
}

#[test]
fn a_self_referential_reference_rewrites_deterministically_without_recursing() {
    let document = self_referential_document();
    let once = flatten(&document);
    assert_eq!(
        once,
        flatten(&document),
        "the same document rewrites the same way"
    );
    assert_eq!(
        once["properties"]["plain"],
        json!({"type": "string"}),
        "the rest of the document still flattens: {once}"
    );
    assert!(
        every_reference_resolves(&once),
        "a reference that survives still resolves: {once}"
    );
}

#[test]
fn mutually_recursive_definitions_rewrite_deterministically_and_stay_resolvable() {
    let document = mutually_recursive_document();
    let once = flatten(&document);
    assert_eq!(
        once,
        flatten(&document),
        "the same document rewrites the same way"
    );
    assert!(
        count(&once, "$ref") > 0,
        "a chain cannot be inlined: {once}"
    );
    assert!(
        count(&once, "$defs") > 0,
        "the definitions a surviving reference names stay beside it: {once}"
    );
    assert!(
        every_reference_resolves(&once),
        "every surviving reference resolves: {once}"
    );
}

#[test]
fn a_nested_definition_is_inlined_so_no_reference_dangles_after_it_is_dropped() {
    let flattened = flatten(&nested_defs_document());
    assert_eq!(
        flattened["properties"]["envelope"]["properties"]["leaf"],
        json!({"type": "string", "description": "an inner leaf"}),
        "the reference into the nested definitions is inlined: {flattened}"
    );
    assert_eq!(
        count(&flattened, "$ref"),
        0,
        "no dangling reference: {flattened}"
    );
    assert_eq!(
        count(&flattened, "$defs"),
        0,
        "no definitions remain: {flattened}"
    );
    assert!(every_reference_resolves(&flattened), "{flattened}");
}

#[test]
fn a_reference_beside_a_sibling_keyword_stays_exactly_as_written() {
    let document = sibling_reference_document();
    let flattened = flatten(&document);
    assert_eq!(
        flattened["properties"]["label"],
        json!({"$ref": "#/$defs/Label", "maxLength": 8}),
        "the sibling keyword survives beside the reference: {flattened}"
    );
    assert_eq!(
        count(&flattened, "$defs"),
        1,
        "the definition the surviving reference names stays beside it: {flattened}"
    );
    assert!(
        every_reference_resolves(&flattened),
        "the surviving reference resolves: {flattened}"
    );
}

#[test]
fn a_rewrite_holds_no_reference_or_the_definitions_it_resolves_against() {
    for document in [
        nested_reference_document(),
        self_referential_document(),
        mutually_recursive_document(),
        nested_defs_document(),
        sibling_reference_document(),
    ] {
        let flattened = flatten(&document);
        assert!(
            count(&flattened, "$ref") == 0 || every_reference_resolves(&flattened),
            "no reference is left without the definitions it resolves against: {flattened}"
        );
    }
}

#[test]
fn the_empty_and_hash_fragments_name_the_document_root() {
    let document = json!({"type": "object", "properties": {}});
    assert_eq!(pointed(&document, ""), Some(&document));
    assert_eq!(pointed(&document, "#"), Some(&document));
}

#[test]
fn a_numeric_token_addresses_an_array_index_only_inside_an_array() {
    let document = json!({
        "tuple": [{"type": "string"}, {"type": "integer"}],
        "0": {"type": "boolean"}
    });
    assert_eq!(
        pointed(&document, "#/tuple/1"),
        Some(&json!({"type": "integer"})),
        "a numeric token is an index inside an array"
    );
    assert_eq!(
        pointed(&document, "#/0"),
        Some(&json!({"type": "boolean"})),
        "a numeric token is a member name inside an object"
    );
    assert_eq!(
        pointed(&document, "#/tuple/7"),
        None,
        "an index past the end"
    );
    assert_eq!(
        pointed(&document, "#/tuple/first"),
        None,
        "a name inside an array"
    );
}

#[test]
fn a_reference_indexing_an_array_inlines_the_indexed_schema() {
    let document = json!({
        "$defs": {"Tuple": [{"type": "string"}, {"type": "integer"}]},
        "type": "object",
        "properties": {"second": {"$ref": "#/$defs/Tuple/1"}}
    });
    let flattened = flatten(&document);
    assert_eq!(
        flattened["properties"]["second"],
        json!({"type": "integer"})
    );
    assert_eq!(count(&flattened, "$ref"), 0, "{flattened}");
}
