// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the config read-back for every harness shape: the JSON
//! key prefixes, the Codex TOML table, the Hermes YAML map, and the judgment
//! of whether an entry names Clio's own executable. Every test writes its own
//! temp config and asserts the parsed entry, never a vendor message.
//!
//! ## Owns
//! - One test per read-back shape and per entry-point judgment
//! - The parse-refusal case for an unparseable JSON config
//!
//! ## Does not own
//! - The shared stubs and fixtures
//!   (`agent_harness_delegate_test_support_tests`)
//! - Running a vendor command (`agent_harness_delegate`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::agent_harness::SERVER_NAME;
use crate::agent_harness_delegate::test_support_tests::{Fixture, record_named};
use std::path::Path;

#[test]
fn missing_config_is_none() {
    let claude = record_named("claude");
    let fixture = Fixture::new("read-missing");
    assert_eq!(
        read_entry(claude, &fixture.config_path(claude), SERVER_NAME, None),
        Ok(None),
        "a file that does not exist has no entry"
    );
}

#[test]
fn claude_json_entry_is_found_and_points_at_the_exe() {
    let claude = record_named("claude");
    let fixture = Fixture::new("read-claude");
    let path = fixture.write_raw(claude, &fixture_entry("mcpServers", &fixture.exe()));

    let entry = read_entry(claude, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("good JSON reads")
        .expect("the clio entry is present");
    assert!(
        entry_points_at(&entry, &fixture.exe()),
        "the entry points at this executable"
    );
}

#[test]
fn a_json_entry_under_another_command_does_not_point_at_the_exe() {
    let claude = record_named("claude");
    let fixture = Fixture::new("read-claude-other");
    let other = fixture.root.join("other");
    let path = fixture.write_raw(claude, &fixture_entry("mcpServers", &other));

    let entry = read_entry(claude, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("good JSON reads")
        .expect("the clio entry is present");
    assert!(
        !entry_points_at(&entry, &fixture.exe()),
        "an entry under another executable is not this one"
    );
}

#[test]
fn opencode_v2_entry_is_found_under_mcp_servers() {
    let opencode = record_named("opencode");
    let fixture = Fixture::new("read-opencode-v2");
    let value = serde_json::json!({ "mcp": { "servers": { "clio": {
        "type": "stdio",
        "command": fixture.exe().to_string_lossy(),
        "args": ["mcp", "stdio"],
    }}}});
    let path = fixture.write_raw(opencode, &value.to_string());

    let entry = read_entry(opencode, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("good JSON reads")
        .expect("the v2 entry is present");
    assert!(entry_points_at(&entry, &fixture.exe()), "{entry:?}");
}

#[test]
fn opencode_v1_entry_is_found_under_the_fallback_key() {
    let opencode = record_named("opencode");
    let fixture = Fixture::new("read-opencode-v1");
    let value = serde_json::json!({ "mcp": { "clio": {
        "type": "stdio",
        "command": fixture.exe().to_string_lossy(),
        "args": ["mcp", "stdio"],
    }}});
    let path = fixture.write_raw(opencode, &value.to_string());

    let entry = read_entry(opencode, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("good JSON reads")
        .expect("the v1 entry is present");
    assert!(entry_points_at(&entry, &fixture.exe()), "{entry:?}");
}

#[test]
fn antigravity_entry_is_found_and_points_at_the_exe() {
    let antigravity = record_named("antigravity");
    let fixture = Fixture::new("read-antigravity");
    let path = fixture.write_raw(antigravity, &fixture_entry("mcpServers", &fixture.exe()));

    let entry = read_entry(antigravity, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("good JSON reads")
        .expect("the clio entry is present");
    assert!(entry_points_at(&entry, &fixture.exe()), "{entry:?}");
}

#[test]
fn codex_toml_table_is_found_and_names_the_exe() {
    let codex = record_named("codex");
    let fixture = Fixture::new("read-codex");
    let path = fixture.write_config_for(codex, &fixture.exe());

    let entry = read_entry(codex, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("the text reads")
        .expect("the clio table is present");
    assert_eq!(
        entry,
        Entry::Text {
            names_exe: true,
            has_server_args: true
        }
    );
    assert!(entry_points_at(&entry, &fixture.exe()));
}

#[test]
fn codex_toml_table_under_another_command_does_not_point_at_the_exe() {
    let codex = record_named("codex");
    let fixture = Fixture::new("read-codex-other");
    let other = fixture.root.join("other");
    let path = fixture.write_config_for(codex, &other);

    let entry = read_entry(codex, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("the text reads")
        .expect("the clio table is present");
    assert_eq!(
        entry,
        Entry::Text {
            names_exe: false,
            has_server_args: true
        }
    );
    assert!(!entry_points_at(&entry, &fixture.exe()));
}

#[test]
fn hermes_yaml_map_is_found_and_names_the_exe() {
    let hermes = record_named("hermes");
    let fixture = Fixture::new("read-hermes");
    let path = fixture.write_config_for(hermes, &fixture.exe());

    let entry = read_entry(hermes, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("the text reads")
        .expect("the clio entry is present");
    assert_eq!(
        entry,
        Entry::Text {
            names_exe: true,
            has_server_args: true
        }
    );
    assert!(entry_points_at(&entry, &fixture.exe()));
}

#[test]
fn hermes_yaml_map_without_the_server_is_none() {
    let hermes = record_named("hermes");
    let fixture = Fixture::new("read-hermes-other");
    let path = fixture.write_raw(hermes, "mcp_servers:\n  other:\n    command: x\n");

    assert_eq!(
        read_entry(hermes, &path, SERVER_NAME, Some(&fixture.exe())),
        Ok(None),
        "another server is not Clio's entry"
    );
}

#[test]
fn codex_toml_exe_under_another_table_does_not_count() {
    let codex = record_named("codex");
    let fixture = Fixture::new("read-codex-foreign-exe");
    let raw = format!(
        "[mcp_servers.other]\ncommand = \"{exe}\"\nargs = [\"mcp\", \"stdio\"]\n\n[mcp_servers.{SERVER_NAME}]\ncommand = \"/elsewhere\"\nargs = [\"mcp\", \"stdio\"]\n",
        exe = fixture.exe().to_string_lossy()
    );
    let path = fixture.write_raw(codex, &raw);

    let entry = read_entry(codex, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("the text reads")
        .expect("the clio table is present");
    assert_eq!(
        entry,
        Entry::Text {
            names_exe: false,
            has_server_args: true
        },
        "the exe belongs to another table, not Clio's"
    );
    assert!(!entry_points_at(&entry, &fixture.exe()));
}

#[test]
fn hermes_yaml_exe_under_another_server_does_not_count() {
    let hermes = record_named("hermes");
    let fixture = Fixture::new("read-hermes-foreign-exe");
    let raw = format!(
        "mcp_servers:\n  other:\n    command: {exe}\n    args:\n      - mcp\n      - stdio\n  {SERVER_NAME}:\n    command: /elsewhere\n    args:\n      - mcp\n      - stdio\n",
        exe = fixture.exe().to_string_lossy()
    );
    let path = fixture.write_raw(hermes, &raw);

    let entry = read_entry(hermes, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("the text reads")
        .expect("the clio entry is present");
    assert_eq!(
        entry,
        Entry::Text {
            names_exe: false,
            has_server_args: true
        },
        "the exe belongs to another server's block, not Clio's"
    );
    assert!(!entry_points_at(&entry, &fixture.exe()));
}

#[test]
fn a_json_entry_naming_the_exe_outside_the_command_does_not_point_at_the_exe() {
    let claude = record_named("claude");
    let fixture = Fixture::new("read-claude-stale-env");
    let other = fixture.root.join("other");
    let value = serde_json::json!({ "mcpServers": { "clio": {
        "type": "stdio",
        "command": other.to_string_lossy(),
        "args": ["mcp", "stdio"],
        "description": format!("managed by {}", fixture.exe().to_string_lossy()),
    }}});
    let path = fixture.write_raw(claude, &value.to_string());

    let entry = read_entry(claude, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("good JSON reads")
        .expect("the clio entry is present");
    assert!(
        !entry_points_at(&entry, &fixture.exe()),
        "an exe named outside the command is a stale registration: {entry:?}"
    );
}

#[test]
fn a_json_command_array_holding_the_server_argv_points_at_the_exe() {
    let claude = record_named("claude");
    let fixture = Fixture::new("read-claude-array");
    let value = serde_json::json!({ "mcpServers": { "clio": {
        "type": "stdio",
        "command": [fixture.exe().to_string_lossy(), "mcp", "stdio"],
    }}});
    let path = fixture.write_raw(claude, &value.to_string());

    let entry = read_entry(claude, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("good JSON reads")
        .expect("the clio entry is present");
    assert!(entry_points_at(&entry, &fixture.exe()), "{entry:?}");
}

#[test]
fn a_json_command_array_for_another_exe_does_not_point_at_the_exe() {
    let claude = record_named("claude");
    let fixture = Fixture::new("read-claude-array-other");
    let other = fixture.root.join("other");
    let value = serde_json::json!({ "mcpServers": { "clio": {
        "type": "stdio",
        "command": [other.to_string_lossy(), "mcp", "stdio"],
    }}});
    let path = fixture.write_raw(claude, &value.to_string());

    let entry = read_entry(claude, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("good JSON reads")
        .expect("the clio entry is present");
    assert!(
        !entry_points_at(&entry, &fixture.exe()),
        "an array naming another executable is not this one: {entry:?}"
    );
}

#[test]
fn codex_toml_block_with_the_exe_but_other_args_does_not_point_at_the_exe() {
    let codex = record_named("codex");
    let fixture = Fixture::new("read-codex-other-args");
    let raw = format!(
        "[mcp_servers.{SERVER_NAME}]\ncommand = \"{exe}\"\nargs = [\"serve\"]\n",
        exe = fixture.exe().to_string_lossy()
    );
    let path = fixture.write_raw(codex, &raw);

    let entry = read_entry(codex, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("the text reads")
        .expect("the clio table is present");
    assert_eq!(
        entry,
        Entry::Text {
            names_exe: true,
            has_server_args: false
        },
        "the exe is there but the server arguments are not"
    );
    assert!(!entry_points_at(&entry, &fixture.exe()));
}

#[test]
fn hermes_yaml_block_with_the_exe_but_other_args_does_not_point_at_the_exe() {
    let hermes = record_named("hermes");
    let fixture = Fixture::new("read-hermes-other-args");
    let raw = format!(
        "mcp_servers:\n  {SERVER_NAME}:\n    command: {exe}\n    args:\n      - serve\n",
        exe = fixture.exe().to_string_lossy()
    );
    let path = fixture.write_raw(hermes, &raw);

    let entry = read_entry(hermes, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("the text reads")
        .expect("the clio entry is present");
    assert_eq!(
        entry,
        Entry::Text {
            names_exe: true,
            has_server_args: false
        },
        "the exe is there but the server arguments are not"
    );
    assert!(!entry_points_at(&entry, &fixture.exe()));
}

#[test]
fn codex_toml_header_with_an_inline_comment_is_found() {
    let codex = record_named("codex");
    let fixture = Fixture::new("read-codex-comment");
    let raw = format!(
        "[mcp_servers.{SERVER_NAME}] # kept by hand\ncommand = \"{exe}\"\nargs = [\"mcp\", \"stdio\"]\n",
        exe = fixture.exe().to_string_lossy()
    );
    let path = fixture.write_raw(codex, &raw);

    let entry = read_entry(codex, &path, SERVER_NAME, Some(&fixture.exe()))
        .expect("the text reads")
        .expect("the commented header is present");
    assert!(entry_points_at(&entry, &fixture.exe()), "{entry:?}");
}

#[test]
fn unparseable_json_is_an_error() {
    let claude = record_named("claude");
    let fixture = Fixture::new("read-unparseable");
    let path = fixture.write_raw(claude, "not json at all");

    let detail = read_entry(claude, &path, SERVER_NAME, None)
        .expect_err("an unparseable file cannot be read back");
    assert!(
        detail.contains("not valid JSON"),
        "the refusal classifies the parse failure: {detail}"
    );
}

/// A JSON config with a top-level `top_key` map holding Clio's entry for
/// `command`.
fn fixture_entry(top_key: &str, command: &Path) -> String {
    let entry = serde_json::json!({
        "type": "stdio",
        "command": command.to_string_lossy(),
        "args": ["mcp", "stdio"],
    });
    let mut servers = serde_json::Map::new();
    servers.insert("clio".to_owned(), entry);
    let mut root = serde_json::Map::new();
    root.insert(top_key.to_owned(), serde_json::Value::Object(servers));
    serde_json::Value::Object(root).to_string()
}
