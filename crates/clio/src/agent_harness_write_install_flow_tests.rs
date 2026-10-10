// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the full-cycle tests of the direct-write
//! install and uninstall merge: install, idempotent reinstall, and uninstall
//! over one realistic config layout holding foreign servers and an unknown
//! top-level key, for the Cursor and GitHub Copilot VS Code profile targets.
//!
//! ## Owns
//! - The `assert_full_cycle_over_a_realistic_layout` fixture and one test per
//!   direct-write target
//!
//! ## Does not own
//! - The install implementation under test (`agent_harness_write_install`)
//! - The shared fixtures in `agent_harness_write_install_tests`
//!
//! ## Boundary
//!
//! Changes to lifecycle behavior should normally require changes here.
//! Changes to refusal, fallback, or permission behavior should not require
//! changes here.
//!
//! Keep this module focused on the full-cycle tests.

use super::tests::{
    Scratch, argv, copilot_profile_config, cursor_config, exe_string, parsed, write_fixture,
};
use super::*;
use std::fs;

/// The full cycle over one realistic config layout: the first install adds
/// only Clio's entry, reinstalling the same argv is idempotent (one Clio
/// entry, a byte-identical file), and the uninstall removes only Clio's
/// entry and keeps every foreign server and unknown top-level key.
fn assert_full_cycle_over_a_realistic_layout(scratch: &Scratch, config: &DirectWriteConfig) {
    let foreign_a = serde_json::json!({"command": "other-a", "args": ["serve"]});
    let foreign_b = serde_json::json!({"type": "stdio", "command": "other-b"});
    let original = serde_json::json!({
        (config.top_level_key): {"other-a": foreign_a, "other-b": foreign_b},
        "unrelated": {"keep": true},
    });
    write_fixture(config, original.to_string());

    let first = install_server_from_file(config, "clio", &argv(scratch, "clio-bin-a"))
        .expect("the first install succeeds");
    assert!(matches!(first, InstallOutcome::Merged { .. }));
    let after_first = parsed(&config.path);
    assert_eq!(
        after_first[config.top_level_key]["clio"]["command"],
        Value::String(exe_string(scratch, "clio-bin-a")),
        "the first install added Clio's entry"
    );
    assert_eq!(
        after_first[config.top_level_key]["other-a"], foreign_a,
        "the first install kept the foreign entries"
    );
    assert_eq!(
        after_first["unrelated"], original["unrelated"],
        "the first install kept the unknown top-level key"
    );
    let installed = fs::read(&config.path).expect("the installed config can be read");

    let reinstall = install_server_from_file(config, "clio", &argv(scratch, "clio-bin-a"))
        .expect("the reinstall succeeds");
    assert!(matches!(reinstall, InstallOutcome::Merged { .. }));
    assert_eq!(
        fs::read(&config.path).expect("the reinstalled config can be read"),
        installed,
        "reinstalling the same argv is idempotent: one Clio entry and nothing else changed"
    );

    let outcome = remove_server_from_direct_write(config, "clio", &argv(scratch, "clio-bin-a"))
        .expect("the uninstall succeeds");
    assert!(matches!(outcome, RemoveOutcome::Removed { .. }));
    let after_uninstall = parsed(&config.path);
    assert!(
        after_uninstall[config.top_level_key].get("clio").is_none(),
        "the uninstall removed Clio's entry"
    );
    assert_eq!(
        after_uninstall[config.top_level_key]["other-a"], foreign_a,
        "the uninstall kept the foreign entries"
    );
    assert_eq!(
        after_uninstall[config.top_level_key]["other-b"], foreign_b,
        "the uninstall kept the foreign entries"
    );
    assert_eq!(
        after_uninstall["unrelated"], original["unrelated"],
        "the uninstall kept the unknown top-level key"
    );
}

#[test]
fn cursor_full_cycle_over_a_realistic_layout_keeps_foreign_entries() {
    let scratch = Scratch::new("cursor-cycle");
    assert_full_cycle_over_a_realistic_layout(&scratch, &cursor_config(&scratch));
}

#[test]
fn copilot_profile_full_cycle_over_a_realistic_layout_keeps_foreign_entries() {
    let scratch = Scratch::new("copilot-cycle");
    assert_full_cycle_over_a_realistic_layout(&scratch, &copilot_profile_config(&scratch));
}
