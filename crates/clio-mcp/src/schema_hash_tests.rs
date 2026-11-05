// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Catalog-hash tests: the published `catalog_hash_definition` is complete and
//! true, a reader can recompute the hash from that definition alone, and the
//! hash covers the tool set, the catalog order, and each tool's fields.
//!
//! Every test is deterministic: the input is the pack this build publishes, no
//! clock, no random seed, no network, and no host value is read.

use super::*;
use serde_json::{Map, Value, json};

/// The published definition object from a freshly built pack.
fn definition() -> Map<String, Value> {
    schema_pack()["catalog_hash_definition"]
        .as_object()
        .expect("the pack publishes catalog_hash_definition as an object")
        .clone()
}

/// One `0x`-prefixed constant read out of the published definition, so the
/// recomputation below uses the published value and not the production one.
fn published_constant(definition: &Map<String, Value>, field: &str) -> u64 {
    let raw = definition
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("definition field `{field}` publishes a hex string"));
    let digits = raw
        .strip_prefix("0x")
        .unwrap_or_else(|| panic!("definition field `{field}` carries a 0x prefix: {raw:?}"));
    u64::from_str_radix(digits, 16)
        .unwrap_or_else(|err| panic!("definition field `{field}` parses as u64: {raw:?} ({err})"))
}

/// FNV-1a 64-bit over `bytes`: XOR each byte in, then multiply by the prime.
fn fnv1a64(offset_basis: u64, prime: u64, bytes: &[u8]) -> u64 {
    let mut hash = offset_basis;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(prime);
    }
    hash
}

/// The pack's `tools[]` array as an owned vector.
fn pack_tools(pack: &Value) -> Vec<Value> {
    pack["tools"]
        .as_array()
        .expect("the pack publishes a tools[] array")
        .clone()
}

/// Hash the pack's tools with `mutate` applied to the first tool, prove the hash
/// changed, then apply `revert` to that same first tool and prove the original
/// hash returns. Nothing is re-cloned between the two assertions, so the second
/// one is a fact about the undo and not about an untouched copy.
fn assert_first_tool_field_change_alters_the_hash(
    mutate: impl Fn(&mut Value),
    revert: impl Fn(&mut Value),
    field: &str,
) {
    let mut tools = pack_tools(&schema_pack());
    let original = catalog_hash(&tools);
    mutate(&mut tools[0]);
    assert_ne!(
        catalog_hash(&tools),
        original,
        "changing `{field}` on the first tool changes catalog_hash"
    );
    revert(&mut tools[0]);
    assert_eq!(
        catalog_hash(&tools),
        original,
        "reverting `{field}` on the same tool restores catalog_hash"
    );
}

/// Protects the published definition's exact key set and values: a field cannot
/// disappear, and none of the ten rules can drift from the documented ones.
///
/// The assertion compares the whole object against the literal below, so a
/// single changed character in any value fails the test. Substring checks
/// cannot carry this weight: an `input` string stripped of "compact UTF-8 JSON
/// encoding" still contains every word a needle list would look for.
#[test]
fn definition_names_algorithm_input_order_and_rendered_form() {
    assert_eq!(
        Value::Object(definition()),
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
        }),
        "catalog_hash_definition is exactly the ten published fields with their exact published values"
    );
}

/// Protects that the definition is sufficient on its own: a reader who follows
/// it, using its published constants and not the production ones, lands exactly
/// on the pack's `catalog_hash`, so definition and implementation cannot
/// silently disagree.
#[test]
fn a_reader_can_recompute_the_hash_from_the_published_definition_alone() {
    let definition = definition();
    let offset_basis = published_constant(&definition, "offset_basis");
    let prime = published_constant(&definition, "prime");
    let pack = schema_pack();
    let mut canonical = Vec::new();
    for tool in pack_tools(&pack) {
        let encoded = serde_json::to_string(&tool).expect("each tool serializes as compact JSON");
        canonical.extend_from_slice(encoded.as_bytes());
    }
    let rendered = format!("fnv1a64:{:016x}", fnv1a64(offset_basis, prime, &canonical));
    assert_eq!(
        rendered,
        pack["catalog_hash"]
            .as_str()
            .expect("catalog_hash is a string"),
        "recomputing from the published definition alone reproduces catalog_hash"
    );
}

/// Protects that pack assembly is deterministic: repeated builds publish one
/// identical hash, so a consumer comparing two packs cannot see false drift.
#[test]
fn repeated_recomputation_returns_the_same_hash() {
    let hashes: Vec<String> = (0..3)
        .map(|_| {
            schema_pack()["catalog_hash"]
                .as_str()
                .expect("catalog_hash is a string")
                .to_owned()
        })
        .collect();
    assert_eq!(hashes[0], hashes[1], "the second build matches the first");
    assert_eq!(hashes[1], hashes[2], "the third build matches the second");
}

/// Protects the rendered form as an external contract: the version prefix, the
/// total width, and the lowercase hex spelling a consumer depends on.
#[test]
fn rendered_form_is_a_version_prefix_and_sixteen_lowercase_hex_digits() {
    let pack = schema_pack();
    let hash = pack["catalog_hash"]
        .as_str()
        .expect("catalog_hash is a string");
    assert!(
        hash.starts_with("fnv1a64:"),
        "the hash carries the version prefix: {hash}"
    );
    assert_eq!(hash.len(), 24, "prefix plus sixteen digits: {hash}");
    let digits = &hash["fnv1a64:".len()..];
    assert!(
        digits
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "sixteen lowercase hex digits: {digits}"
    );
}

/// Protects that the hash covers the tool set itself: a rename, a removal, and
/// an addition each change it, and undoing each edit on the same vector
/// restores the value.
#[test]
fn renaming_adding_or_removing_a_tool_changes_the_hash_and_reverting_restores_it() {
    let pack = schema_pack();
    let mut tools = pack_tools(&pack);
    let original = catalog_hash(&tools);
    assert_eq!(
        original,
        pack["catalog_hash"]
            .as_str()
            .expect("catalog_hash is a string"),
        "the private catalog_hash reproduces the published value"
    );

    let name = tools[0]["name"].clone();
    tools[0]["name"] = json!("__renamed_first_tool__");
    assert_ne!(
        catalog_hash(&tools),
        original,
        "renaming the first tool changes the hash"
    );
    tools[0]["name"] = name;
    assert_eq!(
        catalog_hash(&tools),
        original,
        "restoring the first tool's name restores the hash"
    );

    let last = tools.last().expect("the catalog publishes tools").clone();
    tools.pop().expect("the catalog publishes tools");
    assert_ne!(
        catalog_hash(&tools),
        original,
        "removing the last tool changes the hash"
    );
    tools.push(last);
    assert_eq!(
        catalog_hash(&tools),
        original,
        "re-inserting the last tool restores the hash"
    );

    let mut copy = tools.last().expect("the catalog publishes tools").clone();
    copy["name"] = json!("__appended_tool_copy__");
    tools.push(copy);
    assert_ne!(
        catalog_hash(&tools),
        original,
        "appending a renamed copy of the last tool changes the hash"
    );
    tools.pop().expect("the appended tool");
    assert_eq!(
        catalog_hash(&tools),
        original,
        "removing the appended tool restores the hash"
    );
}

/// Protects that the hash covers each published field of every tool: a change to
/// a description, an input schema, or an output schema alone changes it, and
/// reverting that field restores the original value.
#[test]
fn changing_only_a_description_an_input_schema_or_an_output_schema_changes_the_hash_and_reverting_restores_it()
 {
    let pack = schema_pack();
    let tools = pack_tools(&pack);
    assert!(!tools.is_empty(), "the catalog publishes at least one tool");
    let first = &tools[0];
    assert!(
        first.get("description").and_then(Value::as_str).is_some(),
        "the first tool publishes a description string"
    );
    assert!(
        first["inputSchema"]["properties"].is_object(),
        "the first tool publishes inputSchema.properties as an object"
    );
    assert!(
        first["outputSchema"]["properties"].is_object(),
        "the first tool publishes outputSchema.properties as an object"
    );
    // The third mutation must edit real published data, so every tool in the
    // pack has to carry an output schema for the assertion to mean anything.
    for tool in &tools {
        assert!(
            tool.get("outputSchema")
                .and_then(Value::as_object)
                .is_some(),
            "every published tool carries an outputSchema object: {:?}",
            tool.get("name")
        );
    }

    let description = first["description"].clone();
    assert_first_tool_field_change_alters_the_hash(
        |tool| tool["description"] = json!("a description changed by the catalog-hash test"),
        |tool| tool["description"] = description.clone(),
        "description",
    );
    assert_first_tool_field_change_alters_the_hash(
        |tool| tool["inputSchema"]["properties"]["__hash_probe"] = json!({"type": "string"}),
        |tool| {
            tool["inputSchema"]["properties"]
                .as_object_mut()
                .expect("inputSchema.properties is an object")
                .remove("__hash_probe");
        },
        "inputSchema.properties.__hash_probe",
    );
    assert_first_tool_field_change_alters_the_hash(
        |tool| tool["outputSchema"]["properties"]["__hash_probe"] = json!({"type": "string"}),
        |tool| {
            tool["outputSchema"]["properties"]
                .as_object_mut()
                .expect("outputSchema.properties is an object")
                .remove("__hash_probe");
        },
        "outputSchema.properties.__hash_probe",
    );
}

/// Protects that catalog order is part of the hashed input, so a re-ordered pack
/// cannot keep the same fingerprint.
#[test]
fn catalog_order_is_part_of_the_hashed_input() {
    let mut tools = pack_tools(&schema_pack());
    assert!(
        tools.len() >= 2,
        "the catalog publishes at least two tools to swap"
    );
    let original = catalog_hash(&tools);
    tools.swap(0, 1);
    assert_ne!(
        catalog_hash(&tools),
        original,
        "swapping two adjacent tools changes the hash"
    );
    tools.swap(0, 1);
    assert_eq!(
        catalog_hash(&tools),
        original,
        "restoring the order restores the hash"
    );
}

/// Protects that the hash covers tool content, not just the list of names, so an
/// edit to a description or a schema cannot hide behind an unchanged tool list.
#[test]
fn the_hash_is_not_a_hash_of_names_only() {
    let definition = definition();
    let offset_basis = published_constant(&definition, "offset_basis");
    let prime = published_constant(&definition, "prime");
    let pack = schema_pack();
    let names: String = pack_tools(&pack)
        .iter()
        .map(|tool| {
            tool["name"]
                .as_str()
                .expect("every tool publishes a name string")
                .to_owned()
        })
        .collect();
    let names_only = format!(
        "fnv1a64:{:016x}",
        fnv1a64(offset_basis, prime, names.as_bytes())
    );
    assert_ne!(
        names_only,
        pack["catalog_hash"]
            .as_str()
            .expect("catalog_hash is a string"),
        "hashing the concatenated tool names only gives a different value"
    );
}
