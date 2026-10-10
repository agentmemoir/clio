// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the harness registry: name normalization, name resolution,
//! the supported-name list, the read-back shape, and the pure argv and snippet
//! builders. Every test injects the executable path; no test reads the
//! filesystem, spawns a process, touches the process environment, or asserts a
//! host-specific path. The direct-write config resolution itself is tested in
//! `agent_harness_targets_tests.rs`.
//!
//! ## Owns
//! - Assertions over `normalize`, `resolve`, `supported_names`, `add_argv`,
//!   `remove_argv`, `server_argv`, `manual_snippet`, and the record read-back
//!
//! ## Does not own
//! - The registry implementation under test (`agent_harness`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use std::ffi::OsString;

const FAKE_EXE: &str = "/opt/clio/bin/clio";

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

#[test]
fn resolve_matches_canonical_name_case_insensitively() {
    for name in ["claude", "Claude", "CLAUDE"] {
        let record = resolve(name).expect("claude must resolve");
        assert_eq!(record.name, "claude", "resolving {name}");
    }
}

#[test]
fn resolve_matches_alias_case_insensitively() {
    for name in ["claude-code", "Claude-Code"] {
        let record = resolve(name).expect("alias must resolve");
        assert_eq!(record.name, "claude", "resolving {name}");
    }
}

#[test]
fn resolve_rejects_names_without_a_record() {
    for name in ["gh-copilot", "vscodium", ""] {
        assert!(resolve(name).is_none(), "{name} must not resolve");
    }
}

#[test]
fn claude_record_uses_the_delegate_mechanism() {
    let record = resolve("claude").expect("claude must resolve");
    assert_eq!(record.mechanism, Mechanism::Delegate);
}

#[test]
fn cursor_and_copilot_records_use_the_direct_write_mechanism() {
    for name in ["cursor", "vs code", "github copilot"] {
        let record = resolve(name).expect("direct-write harness must resolve");
        assert_eq!(record.mechanism, Mechanism::DirectWrite, "resolving {name}");
    }
}

#[test]
fn claude_record_names_its_config_file() {
    let record = resolve("claude").expect("claude must resolve");
    assert_eq!(record.config_file, ".claude.json");
}

#[test]
fn antigravity_record_names_the_cli_global_config_file() {
    let record = resolve("antigravity").expect("antigravity must resolve");
    assert_eq!(record.config_file, ".gemini/config/mcp_config.json");
}

#[test]
fn supported_names_lists_the_registry_in_order() {
    assert_eq!(
        supported_names(),
        vec![
            "claude",
            "codex",
            "opencode",
            "antigravity",
            "hermes",
            "cursor",
            "github copilot"
        ]
    );
}

#[test]
fn every_registered_record_resolves() {
    for record in RECORDS {
        let resolved = resolve(record.name).expect("registered record must resolve");
        assert_eq!(resolved.name, record.name, "resolving {}", record.name);
    }
}

#[test]
fn every_registered_record_is_listed_by_supported_names() {
    for record in RECORDS {
        assert!(
            supported_names().contains(&record.name),
            "{} is registered but not listed",
            record.name
        );
    }
}

#[test]
fn every_alias_resolves_to_its_canonical_record() {
    for record in RECORDS {
        for alias in record.aliases {
            let resolved = resolve(alias).expect("registered alias must resolve");
            assert_eq!(resolved.name, record.name, "alias {alias}");
        }
    }
}

#[test]
fn normalize_accepts_every_canonical_name_case_insensitively() {
    for canonical in CANONICAL_NAMES {
        for probe in [
            canonical.to_ascii_lowercase(),
            canonical.to_ascii_uppercase(),
        ] {
            assert_eq!(normalize(&probe), Some(*canonical), "normalizing {probe}");
        }
    }
}

#[test]
fn normalize_accepts_github_copilot_case_insensitively() {
    for name in ["github copilot", "GitHub Copilot", "GITHUB COPILOT"] {
        assert_eq!(
            normalize(name),
            Some("github copilot"),
            "normalizing {name}"
        );
    }
}

#[test]
fn normalize_maps_vs_code_to_github_copilot() {
    for name in ["vs code", "VS Code"] {
        assert_eq!(
            normalize(name),
            Some("github copilot"),
            "normalizing {name}"
        );
    }
}

#[test]
fn github_copilot_resolves_to_its_direct_write_record() {
    let record = resolve("github copilot").expect("github copilot must resolve");
    assert_eq!(record.name, "github copilot");
    assert_eq!(record.mechanism, Mechanism::DirectWrite);
}

#[test]
fn direct_write_records_name_their_home_relative_fallback_file() {
    let cursor = resolve("cursor").expect("cursor must resolve");
    let copilot = resolve("github copilot").expect("github copilot must resolve");
    assert_eq!(cursor.config_file, ".cursor/mcp.json");
    assert_eq!(copilot.config_file, ".copilot/mcp-config.json");
}

#[test]
fn copilot_read_back_checks_both_config_keys() {
    let copilot = resolve("github copilot").expect("github copilot must resolve");
    assert_eq!(
        copilot.read_back,
        ReadBack::Json {
            key_prefixes: &[&["servers"], &["mcpServers"]],
        }
    );
}

#[test]
fn normalize_rejects_names_outside_the_alias_set() {
    for name in ["gh-copilot", ""] {
        assert_eq!(normalize(name), None, "{name} must not normalize");
    }
}

#[test]
fn opencode_read_back_prefers_the_v2_key_then_the_v1_key() {
    let record = resolve("opencode").expect("opencode must resolve");
    assert_eq!(
        record.read_back,
        ReadBack::Json {
            key_prefixes: &[&["mcp", "servers"], &["mcp"]],
        }
    );
}

#[test]
fn text_config_records_declare_their_read_back_shape() {
    let codex = resolve("codex").expect("codex must resolve");
    let hermes = resolve("hermes").expect("hermes must resolve");
    assert_eq!(codex.read_back, ReadBack::TomlServerTable);
    assert_eq!(hermes.read_back, ReadBack::YamlServerMap);
}

#[test]
fn add_argv_builds_the_claude_add_command() {
    let record = resolve("claude").expect("claude must resolve");
    let mut expected = os(&[
        "claude",
        "mcp",
        "add",
        "--transport",
        "stdio",
        "--scope",
        "user",
        "clio-test",
        "--",
    ]);
    expected.push(OsString::from(FAKE_EXE));
    expected.extend(os(&["mcp", "stdio"]));
    assert_eq!(
        record.add_argv("clio-test", Path::new(FAKE_EXE)),
        expected,
        "claude add argv"
    );
}

#[test]
fn remove_argv_builds_the_claude_remove_command() {
    let record = resolve("claude").expect("claude must resolve");
    assert_eq!(
        record.remove_argv("clio-test"),
        os(&["claude", "mcp", "remove", "clio-test", "--scope", "user"]),
        "claude remove argv"
    );
}

#[test]
fn add_argv_builds_the_codex_add_command() {
    let record = resolve("codex").expect("codex must resolve");
    let mut expected = os(&["codex", "mcp", "add", "clio-test", "--"]);
    expected.push(OsString::from(FAKE_EXE));
    expected.extend(os(&["mcp", "stdio"]));
    assert_eq!(
        record.add_argv("clio-test", Path::new(FAKE_EXE)),
        expected,
        "codex add argv"
    );
}

#[test]
fn remove_argv_builds_the_codex_remove_command() {
    let record = resolve("codex").expect("codex must resolve");
    assert_eq!(
        record.remove_argv("clio-test"),
        os(&["codex", "mcp", "remove", "clio-test"]),
        "codex remove argv"
    );
}

#[test]
fn add_argv_builds_the_opencode_add_command() {
    let record = resolve("opencode").expect("opencode must resolve");
    let mut expected = os(&["opencode", "mcp", "add", "--global", "clio-test", "--"]);
    expected.push(OsString::from(FAKE_EXE));
    expected.extend(os(&["mcp", "stdio"]));
    assert_eq!(
        record.add_argv("clio-test", Path::new(FAKE_EXE)),
        expected,
        "opencode add argv"
    );
}

#[test]
fn remove_argv_is_empty_for_opencode() {
    let record = resolve("opencode").expect("opencode must resolve");
    assert!(
        record.remove_argv("clio-test").is_empty(),
        "opencode has no vendor remove command"
    );
}

#[test]
fn add_argv_builds_the_antigravity_add_command() {
    let record = resolve("antigravity").expect("antigravity must resolve");
    let mut expected = os(&["agy", "mcp", "add", "clio-test"]);
    expected.push(OsString::from(FAKE_EXE));
    expected.extend(os(&["mcp", "stdio"]));
    assert_eq!(
        record.add_argv("clio-test", Path::new(FAKE_EXE)),
        expected,
        "antigravity add argv"
    );
}

#[test]
fn remove_argv_builds_the_antigravity_remove_command() {
    let record = resolve("antigravity").expect("antigravity must resolve");
    assert_eq!(
        record.remove_argv("clio-test"),
        os(&["agy", "mcp", "remove", "clio-test"]),
        "antigravity remove argv"
    );
}

#[test]
fn add_argv_builds_the_hermes_add_command() {
    let record = resolve("hermes").expect("hermes must resolve");
    let mut expected = os(&["hermes", "mcp", "add", "clio-test", "--command"]);
    expected.push(OsString::from(FAKE_EXE));
    expected.extend(os(&["--args", "mcp", "stdio"]));
    assert_eq!(
        record.add_argv("clio-test", Path::new(FAKE_EXE)),
        expected,
        "hermes add argv"
    );
}

#[test]
fn remove_argv_builds_the_hermes_remove_command() {
    let record = resolve("hermes").expect("hermes must resolve");
    assert_eq!(
        record.remove_argv("clio-test"),
        os(&["hermes", "mcp", "remove", "clio-test"]),
        "hermes remove argv"
    );
}

#[test]
fn server_argv_builds_exe_then_mcp_stdio() {
    let mut expected = vec![OsString::from(FAKE_EXE)];
    expected.extend(os(&["mcp", "stdio"]));
    assert_eq!(server_argv(Path::new(FAKE_EXE)), expected, "server argv");
}

#[test]
fn manual_snippet_is_the_add_command_on_one_line() {
    let record = resolve("claude").expect("claude must resolve");
    assert_eq!(
        record.manual_snippet(SERVER_NAME, Path::new(FAKE_EXE)),
        "claude mcp add --transport stdio --scope user clio -- /opt/clio/bin/clio mcp stdio",
        "manual snippet"
    );
}

#[test]
fn manual_snippet_quotes_an_executable_path_with_spaces() {
    let record = resolve("claude").expect("claude must resolve");
    let snippet = record.manual_snippet(SERVER_NAME, Path::new("/opt/my tools/clio"));
    assert!(
        snippet.contains("\"/opt/my tools/clio\""),
        "snippet must quote the path: {snippet}"
    );
}

#[test]
fn server_name_is_clio() {
    assert_eq!(SERVER_NAME, "clio");
}
