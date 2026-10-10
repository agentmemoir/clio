// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests direct-write config resolution: the Cursor global
//! config path, the GitHub Copilot VS Code profile path per OS layout, the
//! Copilot fallback selection, and the top-level key and entry shape of each
//! file. Every resolution test injects the environment through
//! `DirectWriteEnv::with`; no test reads the filesystem or asserts a
//! host-specific path. One test calls `DirectWriteEnv::production()` and
//! reads the process environment, asserting the resolved path relative to
//! the host's home instead of a hardcoded one; no test writes the
//! environment.
//!
//! ## Owns
//! - Assertions over `DirectWriteEnv::with`, `direct_write_config`, and the
//!   Copilot profile-versus-fallback selection
//!
//! ## Does not own
//! - The resolution implementation under test (`agent_harness_targets`)
//! - The registry-level record assertions (`agent_harness_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::agent_harness::resolve;
use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use std::path::PathBuf;

const FAKE_HOME: &str = "/home/fake";

/// An injected environment built from plain test strings, so no test touches
/// the real process environment.
fn env(
    os: OsLayout,
    home: &str,
    xdg_config_home: Option<&str>,
    appdata: Option<&str>,
    copilot_home: Option<&str>,
) -> DirectWriteEnv {
    DirectWriteEnv::with(
        os,
        PathBuf::from(home),
        xdg_config_home.map(PathBuf::from),
        appdata.map(PathBuf::from),
        copilot_home.map(PathBuf::from),
    )
}

fn cursor_config(env: &DirectWriteEnv) -> crate::agent_harness_targets::DirectWriteConfig {
    resolve("cursor")
        .expect("cursor must resolve")
        .direct_write_config(env)
        .expect("cursor must resolve a config file")
}

fn copilot_config(env: &DirectWriteEnv) -> Option<crate::agent_harness_targets::DirectWriteConfig> {
    resolve("github copilot")
        .expect("github copilot must resolve")
        .direct_write_config(env)
}

#[test]
fn cursor_resolves_its_global_config_under_the_injected_home() {
    let config = cursor_config(&env(OsLayout::Linux, FAKE_HOME, None, None, None));
    assert_eq!(config.path, PathBuf::from("/home/fake/.cursor/mcp.json"));
    assert_eq!(config.top_level_key, "mcpServers");
    assert_eq!(config.entry_type, None, "cursor entries carry no type");
}

#[test]
fn cursor_resolves_no_config_without_a_home() {
    let config = resolve("cursor")
        .expect("cursor must resolve")
        .direct_write_config(&env(OsLayout::Linux, "", None, None, None));
    assert!(config.is_none(), "no home resolves no cursor config file");
}

#[test]
fn copilot_resolves_the_vs_code_profile_on_macos() {
    let config = copilot_config(&env(OsLayout::Mac, FAKE_HOME, None, None, None))
        .expect("macos home resolves the profile");
    assert_eq!(
        config.path,
        PathBuf::from("/home/fake/Library/Application Support/Code/User/mcp.json")
    );
    assert_eq!(config.top_level_key, "servers");
    assert_eq!(config.entry_type, Some("stdio"));
}

#[test]
fn copilot_prefers_xdg_config_home_on_linux_and_ignores_copilot_home() {
    let config = copilot_config(&env(
        OsLayout::Linux,
        FAKE_HOME,
        Some("/xdg/config"),
        None,
        Some("/fake/copilot"),
    ))
    .expect("an xdg root resolves the profile");
    assert_eq!(config.path, PathBuf::from("/xdg/config/Code/User/mcp.json"));
    assert_eq!(config.top_level_key, "servers");
    assert_eq!(config.entry_type, Some("stdio"));
}

#[test]
fn copilot_profile_defaults_to_the_home_config_dir_on_linux() {
    let config = copilot_config(&env(OsLayout::Linux, FAKE_HOME, None, None, None))
        .expect("a home resolves the profile");
    assert_eq!(
        config.path,
        PathBuf::from("/home/fake/.config/Code/User/mcp.json")
    );
}

#[test]
fn copilot_prefers_appdata_on_windows() {
    let config = copilot_config(&env(
        OsLayout::Windows,
        FAKE_HOME,
        None,
        Some("C:/Users/fake/AppData/Roaming"),
        None,
    ))
    .expect("appdata resolves the profile");
    assert_eq!(
        config.path,
        PathBuf::from("C:/Users/fake/AppData/Roaming/Code/User/mcp.json")
    );
    assert_eq!(config.top_level_key, "servers");
    assert_eq!(config.entry_type, Some("stdio"));
}

#[test]
fn copilot_profile_defaults_to_the_home_appdata_on_windows() {
    let config = copilot_config(&env(OsLayout::Windows, FAKE_HOME, None, None, None))
        .expect("a home resolves the profile");
    assert_eq!(
        config.path,
        PathBuf::from("/home/fake/AppData/Roaming/Code/User/mcp.json")
    );
}

#[test]
fn copilot_falls_back_to_copilot_home_when_the_profile_is_unresolvable() {
    let config = copilot_config(&env(OsLayout::Linux, "", None, None, Some("/fake/copilot")))
        .expect("copilot home resolves the fallback");
    assert_eq!(config.path, PathBuf::from("/fake/copilot/mcp-config.json"));
    assert_eq!(config.top_level_key, "mcpServers");
    assert_eq!(config.entry_type, Some("local"));
}

#[test]
fn copilot_fallback_dir_defaults_to_the_home_dot_copilot_dir() {
    let dir = env(OsLayout::Linux, FAKE_HOME, Some("/xdg"), None, None)
        .copilot_fallback_dir()
        .expect("a home resolves the default fallback dir");
    assert_eq!(dir, PathBuf::from("/home/fake/.copilot"));
    assert_eq!(
        env(
            OsLayout::Linux,
            FAKE_HOME,
            Some("/xdg"),
            None,
            Some("/fake/copilot")
        )
        .copilot_fallback_dir(),
        Some(PathBuf::from("/fake/copilot")),
        "copilot home overrides the default"
    );
}

#[test]
fn copilot_resolves_no_config_without_any_resolvable_directory() {
    assert!(
        copilot_config(&env(OsLayout::Mac, "", None, None, None)).is_none(),
        "no home and no copilot home resolves nothing"
    );
}

#[test]
fn a_delegate_harness_resolves_no_direct_write_config() {
    let env = env(OsLayout::Linux, FAKE_HOME, None, None, None);
    let record = resolve("claude").expect("claude must resolve");
    assert!(record.direct_write_config(&env).is_none());
}

#[test]
fn production_resolves_cursor_relative_to_the_process_home() {
    let env = DirectWriteEnv::production();
    let config = resolve("cursor")
        .expect("cursor must resolve")
        .direct_write_config(&env);
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match home {
        Some(home) => {
            let config = config.expect("a home directory resolves the cursor config file");
            assert_eq!(
                config.path,
                PathBuf::from(home).join(".cursor").join("mcp.json")
            );
            assert_eq!(config.top_level_key, "mcpServers");
        }
        None => assert!(
            config.is_none(),
            "no home directory resolves no cursor config file"
        ),
    }
}
