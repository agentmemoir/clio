// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the type half of the MCP argument contract: a value, or an
//! array entry, whose JSON type the published schema does not accept is refused
//! before the tool runs, and the runtime arguments are checked whether or not a
//! schema declares them.
//!
//! ## Owns
//! - The schema-driven sweep over every published tool, property type, and entry type
//! - The negative control that the types a schema declares are accepted
//! - The guard that a published property keeps a declared type unless it is listed
//!
//! ## Does not own
//! - Argument names, required arguments, and blank strings (see `arg_contract_tests`)
//! - The tool handlers' own value rules (see each tool's test module)
//!
//! ## Boundary
//!
//! Changes to which value types a tool accepts should normally require changes to
//! its schema and its rows below. Changes to the type rules should normally
//! require changes here.
//!
//! Keep this module focused on the type rules.

use super::check;
use serde_json::{Map, Value, json};

/// Properties the published pack leaves untyped, and why each one may be. A
/// property the contract cannot check is a property whose wrong type is dropped,
/// so every other published property must declare a type: a later edit that
/// removes one fails this suite instead of quietly ending the check for that
/// argument. An entry that is no longer untyped fails too, so the list can only
/// shrink.
const UNTYPED_ON_PURPOSE: &[(&str, &str, &str)] = &[
    (
        "config_set",
        "value",
        "any JSON value; the handler validates it against the dotted path",
    ),
    (
        "correct",
        "new_value",
        "a string, or any JSON value whose serialized form replaces the snapshot",
    ),
    (
        "sync_status",
        "sync_key",
        "one shared secret or a newest-first array of them",
    ),
];

/// One JSON value of each type the catalog declares.
fn sample(type_name: &str) -> Option<Value> {
    match type_name {
        "string" => Some(json!("x")),
        "number" => Some(json!(1)),
        "boolean" => Some(json!(true)),
        "array" => Some(json!([])),
        "object" => Some(json!({})),
        _ => None,
    }
}

/// The `type` names a schema node declares, in the string form or the array
/// form. A node with no `type` declares none, and is not type-checked.
fn declared_names(node: &Value) -> Vec<&str> {
    match node.get("type") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// A value the declared type set admits, and one it does not.
fn typed_pair(names: &[&str]) -> Option<(Value, Value)> {
    let good = names.iter().find_map(|name| sample(name))?;
    let bad = ["string", "boolean", "number", "object", "array"]
        .iter()
        .find(|name| !names.contains(name))
        .and_then(|name| sample(name))?;
    Some((good, bad))
}

/// The arguments a tool's schema requires, each typed as that schema types it. A
/// required property the schema leaves untyped carries null, which every
/// argument reader in this crate reads as absent.
fn required_args(schema: &Value) -> Map<String, Value> {
    let properties = schema["properties"].as_object();
    let mut args = Map::new();
    for required in schema["required"].as_array().into_iter().flatten() {
        let Some(key) = required.as_str() else {
            continue;
        };
        let value = properties
            .and_then(|properties| properties.get(key))
            .and_then(|property| property["type"].as_str())
            .and_then(sample)
            .unwrap_or(Value::Null);
        args.insert(key.to_owned(), value);
    }
    args
}

#[test]
fn a_wrong_typed_optional_argument_is_refused_by_name() {
    // Reported over MCP stdio: a string `limit` read as absent, so the call
    // returned the default page and reported success.
    let error = check("retrieve", &json!({"query": "q", "limit": "not_a_number"}))
        .expect_err("a string limit must be refused");
    assert_eq!(error.code.as_str(), "invalid_type");
    assert_eq!(error.message, "limit must be a number");
    // A whole number is what the schema types `limit` as, and it still passes.
    assert!(check("retrieve", &json!({"query": "q", "limit": 7})).is_ok());
}

#[test]
fn an_explicit_null_reads_as_absent_and_is_not_type_checked() {
    // Every argument reader in this crate reads an explicit null as absent, so
    // null stays a documented way to take the default rather than a type fault.
    assert!(check("retrieve", &json!({"query": "q", "limit": null})).is_ok());
}

#[test]
fn a_polymorphic_argument_keeps_the_forms_its_handler_accepts() {
    // `correct` serializes any JSON value into the corrected gist/snapshot, and
    // `update` stringifies a number. The published schema has to admit both
    // forms, or the contract would refuse a call the handler serves.
    assert!(
        check(
            "correct",
            &json!({"item_id": "i", "reason": "r", "new_value": {"meeting": "3pm"}})
        )
        .is_ok()
    );
    assert!(
        check(
            "update",
            &json!({"target": "t", "new_value": 0.8, "update_rule": "continuous"})
        )
        .is_ok()
    );
    // The declared set is enforced narrowly: a type it excludes is still refused.
    let error = check(
        "update",
        &json!({"target": "t", "new_value": {}, "update_rule": "continuous"}),
    )
    .expect_err("an object new_value must be refused");
    assert_eq!(error.message, "new_value must be a string or a number");
}

#[test]
fn a_wrongly_typed_runtime_argument_is_refused_on_every_tool() {
    // `bank`, `actor` and `timestamp` are read by the runtime for every tool and
    // most schemas declare none of them, so a wrongly typed one used to be read
    // as absent: `resolve_ctx` fell back to the connection default bank and
    // `resolve_timestamp` to the moment of the call, and the write still reported
    // success. The contract now types them whether or not a tool declares them.
    let published = crate::protocol::McpHandler::tools_list();
    let tools = published["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty(), "the published pack must not be empty");
    let mut checked = 0_usize;
    for tool in tools {
        let name = tool["name"].as_str().expect("tool name");
        let schema = &tool["inputSchema"];
        for (key, wrong) in [
            ("bank", json!(7)),
            ("actor", json!(true)),
            ("timestamp", json!(1_700_000_000)),
        ] {
            let mut args = required_args(schema);
            args.insert(key.to_owned(), wrong);
            let error = check(name, &Value::Object(args))
                .expect_err(&format!("{name}: a wrongly typed `{key}` must be refused"));
            assert_eq!(
                error.code.as_str(),
                "invalid_type",
                "{name}.{key}: {}",
                error.message
            );
            assert_eq!(error.message, format!("{key} must be a string"), "{name}");
            checked += 1;
        }
    }
    assert_eq!(
        checked,
        tools.len() * 3,
        "every published tool must type-check all three runtime arguments"
    );
}

#[test]
fn an_array_entry_of_the_wrong_type_is_refused_by_name() {
    // `retrieve`'s `domains` is an array of strings, and `str_list` skipped every
    // other entry, so one mistyped element narrowed the filter and the call
    // reported success.
    let error = check(
        "retrieve",
        &json!({"query": "q", "domains": ["semantic", 7]}),
    )
    .expect_err("a non-string domain must be refused");
    assert_eq!(error.code.as_str(), "invalid_type");
    assert_eq!(error.message, "domains[1] must be a string");
    assert!(check("retrieve", &json!({"query": "q", "domains": ["semantic"]})).is_ok());
    assert!(check("retrieve", &json!({"query": "q", "domains": []})).is_ok());
}

#[test]
fn every_published_property_and_entry_type_is_enforced() {
    let published = crate::protocol::McpHandler::tools_list();
    let tools = published["tools"].as_array().expect("tools array").clone();
    assert!(!tools.is_empty(), "the published pack must not be empty");
    let mut typed_tools = 0_usize;
    let mut refusals = 0_usize;
    for tool in &tools {
        let name = tool["name"].as_str().expect("tool name");
        let props = tool["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} publishes no properties"));
        // Carry a value each property's own type admits — null for a property
        // the schema deliberately leaves untyped, so it still reads as present —
        // so when one property is flipped the only fault left is the type under
        // test.
        let good: Map<String, Value> = props
            .iter()
            .map(|(key, prop)| {
                let value = typed_pair(&declared_names(prop)).map_or(Value::Null, |(ok, _)| ok);
                (key.clone(), value)
            })
            .collect();
        // The negative control. A call built only from what the schema declares
        // is a call the contract accepts, so a declared type the rest of the
        // rules cannot satisfy fails here instead of passing unnoticed.
        check(name, &Value::Object(good.clone())).unwrap_or_else(|error| {
            panic!(
                "{name}: the types its own schema declares were refused: {}",
                error.message
            )
        });
        let mut tool_refusals = 0_usize;
        for (key, prop) in props {
            let Some((_, bad)) = typed_pair(&declared_names(prop)) else {
                continue;
            };
            let mut args = good.clone();
            args.insert(key.clone(), bad);
            let error = check(name, &Value::Object(args))
                .expect_err(&format!("{name}: a mistyped `{key}` must be refused"));
            assert_eq!(
                error.code.as_str(),
                "invalid_type",
                "{name}.{key}: {}",
                error.message
            );
            assert!(
                error.message.starts_with(&format!("{key} must be ")),
                "{name}.{key}: {}",
                error.message
            );
            tool_refusals += 1;
            // The entry rule too: an array the schema types by `items` is given
            // one entry whose type the schema excludes.
            let Some((_, bad_entry)) = typed_pair(&declared_names(&prop["items"])) else {
                continue;
            };
            let mut entry_args = good.clone();
            entry_args.insert(key.clone(), Value::Array(vec![bad_entry]));
            let error = check(name, &Value::Object(entry_args))
                .expect_err(&format!("{name}: a mistyped `{key}[0]` must be refused"));
            assert_eq!(
                error.code.as_str(),
                "invalid_type",
                "{name}.{key}[0]: {}",
                error.message
            );
            assert!(
                error.message.starts_with(&format!("{key}[0] must be ")),
                "{name}.{key}[0]: {}",
                error.message
            );
            tool_refusals += 1;
        }
        if tool_refusals > 0 {
            typed_tools += 1;
            refusals += tool_refusals;
        }
    }
    // A handful of published views declare no property at all, so the sweep
    // cannot cover every tool; it must still cover the rest of the catalog,
    // which declares far more typed properties than there are tools.
    assert!(
        refusals >= tools.len(),
        "the sweep must exercise the type rule across the catalog: {refusals} refusal(s) over \
         {typed_tools} of {} tool(s)",
        tools.len()
    );
}

#[test]
fn every_published_property_declares_a_type_unless_it_is_listed() {
    // The contract checks only what a schema types, so removing one `type` ends
    // the check for that argument and nothing reports the loss.
    let published = crate::protocol::McpHandler::tools_list();
    let tools = published["tools"].as_array().expect("tools array");
    let mut untyped: Vec<(String, String)> = Vec::new();
    for tool in tools {
        let name = tool["name"].as_str().expect("tool name");
        let props = tool["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} publishes no properties"));
        for (key, prop) in props {
            if prop.get("type").is_none() {
                untyped.push((name.to_owned(), key.clone()));
            }
        }
    }
    let mut listed: Vec<(String, String)> = UNTYPED_ON_PURPOSE
        .iter()
        .map(|(tool, key, _)| ((*tool).to_owned(), (*key).to_owned()))
        .collect();
    untyped.sort();
    listed.sort();
    assert_eq!(
        untyped, listed,
        "a published property must declare a type unless it is one of the listed ones"
    );
}
