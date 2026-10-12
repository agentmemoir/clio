// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `help --json` catalog document's shape: the current
//! format version and key set, the command-spec serialization with its
//! resolved flag data, the shared/global flags, the derived groups, the pack
//! fields carried through, and the CLI metadata hash.
//!
//! ## Owns
//! - Assertions over the catalog document and `metadata_hash`
//!
//! ## Does not own
//! - The document assembly under test (the `catalog` submodule)
//! - The binding and unbound-command projections' own invariants
//!   (`cli_help_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;

/// One command's serialized record from the catalog, by name.
fn spec_entry(name: &str) -> Value {
    let document = catalog();
    let found = document["command_specs"]
        .as_array()
        .expect("command_specs is an array")
        .iter()
        .find(|spec| spec["name"] == name)
        .unwrap_or_else(|| panic!("{name} serializes"));
    found.clone()
}

/// One flag of a serialized record, by long spelling.
fn flag_of(entry: &Value, long: &str) -> Value {
    let found = entry["flags"]
        .as_array()
        .expect("flags is an array")
        .iter()
        .find(|flag| flag["long"] == long)
        .unwrap_or_else(|| panic!("--{long} serializes"));
    found.clone()
}

/// The catalog emits exactly the current format version: the constant is the
/// one active version, and the document carries it and no other.
#[test]
fn the_catalog_emits_only_the_current_format_version() {
    assert_eq!(CATALOG_FORMAT_VERSION, 2);
    assert_eq!(catalog()["format_version"], CATALOG_FORMAT_VERSION);
}

/// The current document holds exactly these top-level keys, in the sorted
/// order `serde_json` emits, so a reader's key walk meets no v1 key and no
/// surprise one.
#[test]
fn the_catalog_holds_exactly_the_current_document_keys() {
    let document = catalog();
    let keys: Vec<&str> = document
        .as_object()
        .expect("the catalog is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "catalog_hash",
            "cli_hash",
            "command_specs",
            "commands",
            "commands_without_tools",
            "format",
            "format_version",
            "groups",
            "mcp_protocol_revision",
            "shared_global",
            "tools",
        ]
    );
}

/// `command_specs` is every registry record, in registry order, with no extra
/// or missing name: the spec projection and the registry are one list.
#[test]
fn command_specs_are_every_registry_record_in_registry_order() {
    let document = catalog();
    let names: Vec<&str> = document["command_specs"]
        .as_array()
        .expect("command_specs is an array")
        .iter()
        .map(|spec| spec["name"].as_str().expect("a command name"))
        .collect();
    let registry: Vec<&str> = crate::cli_command_registry::names().collect();
    assert_eq!(names, registry);
}

/// One record's serialized field set is exactly the record model's fields, and
/// `get` pins the resolved flag data: the `--output` global keeps its short
/// spelling, its enum kind, and its allowed values, so a reader never
/// re-resolves them.
#[test]
fn a_command_spec_serializes_every_record_field() {
    let entry = spec_entry("get");
    let fields: Vec<&str> = entry
        .as_object()
        .expect("a serialized record is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        vec![
            "accepted_extra",
            "aliases",
            "examples",
            "flag_aliases",
            "flags",
            "name",
            "notes",
            "positionals",
            "section",
            "summary",
            "tool",
            "usage_alternates",
            "usage_override",
        ]
    );
    let spec = crate::cli_command_registry::find("get").unwrap();
    assert_eq!(entry["aliases"], json!(["show"]));
    assert_eq!(entry["tool"], json!(spec.tool));
    assert_eq!(entry["section"], json!("read"));
    assert_eq!(entry["accepted_extra"], json!(spec.accepted_extra));
    let output = flag_of(&entry, "output");
    assert_eq!(output["short"], json!("o"));
    assert_eq!(
        output["value"],
        json!({ "placeholder": "json|text", "kind": "enum", "allowed": ["json", "text"] })
    );
    assert_eq!(output["default"], Value::Null);
}

/// The override, alternate, default, hidden-alias, optional-value, and
/// exclusive-group fields keep their meaning on the records that set them, so
/// a reader reads the record's data and not an erased null.
#[test]
fn override_alternate_default_and_alias_fields_keep_their_meaning() {
    let add = spec_entry("auth token add");
    let record = crate::cli_command_registry::find("auth token add").unwrap();
    assert_eq!(add["usage_override"], json!(record.usage_override));
    assert!(
        add["usage_override"].as_str().is_some(),
        "the usage override was erased"
    );
    assert_eq!(flag_of(&add, "bank")["required"], json!(true));
    assert_eq!(flag_of(&add, "bank")["repeatable"], json!(true));

    let admit = spec_entry("admit");
    let admit_record = crate::cli_command_registry::find("admit").unwrap();
    assert_eq!(
        admit["usage_alternates"],
        json!(admit_record.usage_alternates)
    );
    assert_ne!(
        admit["usage_alternates"].as_array().unwrap().as_slice(),
        [] as [Value; 0]
    );

    let limit = flag_of(&spec_entry("recall"), "limit");
    assert_eq!(limit["default"], json!("10"));
    assert_eq!(limit["value"]["kind"], json!("uint"));

    let export = spec_entry("mcp schema-export");
    assert_eq!(
        export["flag_aliases"],
        json!([{ "alias": "path", "canonical": "out" }])
    );
    assert_eq!(
        flag_of(&spec_entry("mcp http"), "bind")["value_optional"],
        json!(true)
    );
    assert_eq!(
        flag_of(&spec_entry("get"), "snapshot")["exclusive_group"],
        json!("snapshot-gist")
    );
}

/// `shared_global` lists the declared global flags in merge order with their
/// resolved value data, so a reader reads the globals without re-deriving
/// them from the record set.
#[test]
fn shared_global_lists_the_declared_global_flags_in_merge_order() {
    let document = catalog();
    let shared = document["shared_global"].clone();
    let longs = |key: &str| -> Vec<String> {
        shared[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} is an array"))
            .iter()
            .map(|flag| flag["long"].as_str().expect("a flag name").to_owned())
            .collect()
    };
    let value_names: Vec<String> = crate::cli_command_registry::global_value_names()
        .map(str::to_owned)
        .collect();
    let bool_names: Vec<String> = crate::cli_command_registry::global_bool_names()
        .map(str::to_owned)
        .collect();
    assert_eq!(longs("value_flags"), value_names);
    assert_eq!(longs("bool_flags"), bool_names);
    let output = shared["value_flags"]
        .as_array()
        .expect("value_flags is an array")
        .iter()
        .find(|flag| flag["long"] == "output")
        .expect("the output global serializes");
    assert_eq!(output["short"], json!("o"));
    assert_eq!(output["value"]["kind"], json!("enum"));
}

/// The catalog's `groups` array is the derived group table, in derived order,
/// and every prefix and entry names a command `clio help <topic>` reaches: an
/// unknown group name would advertise a command the dispatcher rejects.
#[test]
fn groups_are_the_derived_table_and_name_only_known_commands() {
    let document = catalog();
    let groups = document["groups"].as_array().expect("groups is an array");
    assert_eq!(groups.len(), crate::cli_help::all_groups().count());
    for (entry, (prefix, subs, shared)) in groups.iter().zip(crate::cli_help::all_groups()) {
        assert_eq!(entry["prefix"], json!(prefix));
        assert_eq!(entry["subcommands"], json!(subs));
        assert_eq!(entry["shared"], json!(shared));
        assert!(
            crate::cli_help::is_known_command(prefix),
            "group prefix `{prefix}` is not a known command"
        );
        for sub in subs {
            let key = format!("{prefix} {sub}");
            assert!(
                crate::cli_help::is_known_command(&key),
                "group entry `{key}` is not a known command"
            );
        }
    }
}

/// The CLI metadata hash is deterministic for one payload and reacts when the
/// metadata changes: two payloads that differ in one alias hash differently,
/// and the same payload hashes to the same digest on every call.
#[test]
fn the_cli_hash_is_deterministic_and_reacts_to_metadata_changes() {
    let payload = |alias: &str| {
        json!({
            "command_specs": [{ "name": "x", "aliases": [alias] }],
            "commands": [],
            "commands_without_tools": [],
            "groups": [],
            "shared_global": {},
        })
    };
    assert_eq!(metadata_hash(&payload("a")), metadata_hash(&payload("a")));
    assert_ne!(metadata_hash(&payload("a")), metadata_hash(&payload("b")));
    assert!(metadata_hash(&payload("a")).starts_with("fnv1a64:"));
}

/// The emitted `cli_hash` covers exactly the emitted CLI metadata: rebuilding
/// the payload from the catalog's own five metadata fields reproduces the
/// digest, so the hash and the fields it names cannot drift apart.
#[test]
fn the_emitted_cli_hash_covers_the_emitted_metadata() {
    let document = catalog();
    let rebuilt = json!({
        "command_specs": document["command_specs"].clone(),
        "commands": document["commands"].clone(),
        "commands_without_tools": document["commands_without_tools"].clone(),
        "groups": document["groups"].clone(),
        "shared_global": document["shared_global"].clone(),
    });
    assert_eq!(document["cli_hash"], json!(metadata_hash(&rebuilt)));
}

/// The pack's protocol revision and catalog hash are carried through
/// unchanged, so the CLI catalog and the MCP schema pack cannot drift apart.
#[test]
fn the_catalog_carries_the_pack_revision_and_hash_through() {
    let pack = clio_mcp::schema::schema_pack();
    let document = catalog();
    assert_eq!(
        document["mcp_protocol_revision"],
        pack["mcp_protocol_revision"]
    );
    assert_eq!(document["catalog_hash"], pack["catalog_hash"]);
}
