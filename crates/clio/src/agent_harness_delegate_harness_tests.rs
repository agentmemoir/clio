// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the delegate runner against the four vendor-CLI harnesses
//! added beside Claude: Codex, `OpenCode`, Antigravity, and Hermes. Every spawn
//! test injects a temp home and a stub vendor binary named after that
//! harness's command, so no test reads the host's real `PATH`, `HOME`, or any
//! real harness config. Each stub records the argument vector it received and
//! the tests assert that vector, never vendor prose.
//!
//! ## Owns
//! - The per-harness install and uninstall scenarios and their recorded-argv
//!   assertions
//! - The Hermes piped confirmation and cancel-without-save cases
//! - The `OpenCode` direct-edit uninstall cases through the delegate runner
//!
//! ## Does not own
//! - The shared stubs and fixtures
//!   (`agent_harness_delegate_test_support_tests`)
//! - The delegate implementation under test (`agent_harness_delegate`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::test_support_tests::{Fixture, expected_args, record_named};
use super::*;
use crate::agent_harness::SERVER_NAME;
use std::fs;

#[test]
fn codex_install_without_the_binary_refuses_with_the_snippet() {
    let codex = record_named("codex");
    let fixture = Fixture::new("codex-absent");
    let err = install(codex, SERVER_NAME, &fixture.exe(), &fixture.empty_env())
        .expect_err("a missing codex must refuse");
    let DelegateError::BinaryMissing {
        binary,
        manual_snippet,
    } = err
    else {
        panic!("expected BinaryMissing, got {err:?}")
    };
    assert_eq!(binary, "codex");
    assert_eq!(
        manual_snippet,
        codex.manual_snippet(SERVER_NAME, &fixture.exe())
    );
}

#[test]
fn codex_binary_that_resolves_but_cannot_run_refuses_with_the_snippet() {
    let codex = record_named("codex");
    let fixture = Fixture::new("codex-unspawnable");
    let stand_in = fixture.root.join("not-an-executable");
    fs::create_dir_all(&stand_in).expect("the stand-in path can be created");
    let env = fixture.env_with_lookup(move |_| Some(stand_in.clone()));
    let err = install(codex, SERVER_NAME, &fixture.exe(), &env)
        .expect_err("a lookup hit that cannot be spawned must refuse");
    let DelegateError::BinaryMissing {
        binary,
        manual_snippet,
    } = err
    else {
        panic!("expected BinaryMissing, got {err:?}")
    };
    assert_eq!(binary, "codex");
    assert_eq!(
        manual_snippet,
        codex.manual_snippet(SERVER_NAME, &fixture.exe())
    );
}

#[cfg(unix)]
#[test]
fn codex_install_writes_and_verifies_the_toml_table() {
    let codex = record_named("codex");
    let mut fixture = Fixture::new("codex-install");
    let env = fixture
        .stub_env_vendor_for(codex, false)
        .expect("the stub needs a directory that permits exec");
    let message = install(codex, SERVER_NAME, &fixture.exe(), &env)
        .expect("the stub wrote the table the read-back accepts");
    assert_eq!(message, "installed");
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&codex.add_argv(SERVER_NAME, &fixture.exe()))
    );
}

#[cfg(unix)]
#[test]
fn codex_zero_exit_without_an_entry_is_a_verification_failure() {
    let codex = record_named("codex");
    let mut fixture = Fixture::new("codex-zero-lies");
    let env = fixture
        .stub_env_for(codex, 0)
        .expect("the stub needs a directory that permits exec");
    let err = install(codex, SERVER_NAME, &fixture.exe(), &env)
        .expect_err("the read-back must not trust a zero exit");
    assert!(
        matches!(err, DelegateError::VerificationFailed { .. }),
        "{err:?}"
    );
}

#[cfg(unix)]
#[test]
fn codex_uninstall_runs_the_remove_argv_and_accepts_an_absent_entry() {
    let codex = record_named("codex");
    let mut fixture = Fixture::new("codex-uninstall");
    let env = fixture
        .stub_env_for(codex, 1)
        .expect("the stub needs a directory that permits exec");
    let message =
        uninstall(codex, SERVER_NAME, &env).expect("a refused remove with no entry is a success");
    assert_eq!(message, "nothing to remove");
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&codex.remove_argv(SERVER_NAME))
    );
}

#[test]
fn opencode_install_without_the_binary_refuses_with_the_snippet() {
    let opencode = record_named("opencode");
    let fixture = Fixture::new("opencode-absent");
    let err = install(opencode, SERVER_NAME, &fixture.exe(), &fixture.empty_env())
        .expect_err("a missing opencode must refuse");
    let DelegateError::BinaryMissing {
        binary,
        manual_snippet,
    } = err
    else {
        panic!("expected BinaryMissing, got {err:?}")
    };
    assert_eq!(binary, "opencode");
    assert_eq!(
        manual_snippet,
        opencode.manual_snippet(SERVER_NAME, &fixture.exe())
    );
}

#[cfg(unix)]
#[test]
fn opencode_install_writes_and_verifies_the_v2_entry() {
    let opencode = record_named("opencode");
    let mut fixture = Fixture::new("opencode-install");
    let env = fixture
        .stub_env_vendor_for(opencode, false)
        .expect("the stub needs a directory that permits exec");
    let message = install(opencode, SERVER_NAME, &fixture.exe(), &env)
        .expect("the stub wrote the mcp.servers entry the read-back accepts");
    assert_eq!(message, "installed");
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&opencode.add_argv(SERVER_NAME, &fixture.exe()))
    );
}

#[test]
fn opencode_uninstall_removes_only_clios_entry() {
    let opencode = record_named("opencode");
    let fixture = Fixture::new("opencode-uninstall");
    let foreign = serde_json::json!({"type": "stdio", "command": "other", "args": ["serve"]});
    let original = serde_json::json!({
        "mcp": { "servers": {
            "clio": { "type": "stdio", "command": fixture.exe().to_string_lossy(), "args": ["mcp", "stdio"] },
            "other": foreign,
        }}
    });
    let path = fixture.write_raw(opencode, &original.to_string());

    let message = uninstall(opencode, SERVER_NAME, &fixture.empty_env())
        .expect("the direct edit removes Clio's entry");
    assert!(
        message.starts_with("removed (backup: "),
        "the success message names the backup: {message}"
    );
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("the edited file can be read"))
            .expect("the edited file is valid JSON");
    assert!(
        after["mcp"]["servers"].get("clio").is_none(),
        "Clio's entry must be gone: {after}"
    );
    assert_eq!(
        after["mcp"]["servers"]["other"], foreign,
        "the foreign server must keep its parsed value"
    );
}

#[test]
fn opencode_uninstall_message_names_the_backup_file() {
    let opencode = record_named("opencode");
    let fixture = Fixture::new("opencode-backup-named");
    let original = serde_json::json!({
        "mcp": { "servers": {
            "clio": { "type": "stdio", "command": fixture.exe().to_string_lossy(), "args": ["mcp", "stdio"] },
        }}
    });
    fixture.write_raw(opencode, &original.to_string());

    let message = uninstall(opencode, SERVER_NAME, &fixture.empty_env())
        .expect("the direct edit removes Clio's entry");
    let backups = fixture.backup_siblings(opencode);
    assert_eq!(backups.len(), 1, "exactly one backup is made");
    assert!(
        message.contains(&backups[0].display().to_string()),
        "the success message must name the backup path: {message}"
    );
}

#[cfg(unix)]
#[test]
fn codex_install_ignores_an_exe_under_another_server() {
    let codex = record_named("codex");
    let mut fixture = Fixture::new("codex-foreign-exe");
    let raw = format!(
        "[mcp_servers.other]\ncommand = \"{exe}\"\nargs = [\"mcp\", \"stdio\"]\n\n[mcp_servers.{SERVER_NAME}]\ncommand = \"/elsewhere\"\nargs = [\"mcp\", \"stdio\"]\n",
        exe = fixture.exe().to_string_lossy()
    );
    fixture.write_raw(codex, &raw);
    let env = fixture
        .stub_env_for(codex, 0)
        .expect("the stub needs a directory that permits exec");
    let err = install(codex, SERVER_NAME, &fixture.exe(), &env)
        .expect_err("an exe under another table must not verify Clio's entry");
    assert!(
        matches!(err, DelegateError::VerificationFailed { .. }),
        "{err:?}"
    );
}

#[test]
fn opencode_uninstall_refuses_an_unparseable_config_byte_identical() {
    let opencode = record_named("opencode");
    let fixture = Fixture::new("opencode-unparseable");
    let raw = "{ // comment\n\"mcp\": { \"servers\": {} }\n}\n";
    let path = fixture.write_raw(opencode, raw);

    let err = uninstall(opencode, SERVER_NAME, &fixture.empty_env())
        .expect_err("an unparseable config is refused");
    let DelegateError::VerificationFailed { detail } = err else {
        panic!("expected VerificationFailed, got {err:?}")
    };
    assert!(
        detail.contains(&path.display().to_string()),
        "the refusal names the file: {detail}"
    );
    assert!(
        detail.contains("remove the entry by hand"),
        "the refusal tells the user what to do: {detail}"
    );
    assert_eq!(
        fs::read(&path).expect("the file can be read"),
        raw.as_bytes(),
        "the bytes must not change"
    );
}

#[test]
fn opencode_uninstall_without_clio_is_a_clear_no_op() {
    let opencode = record_named("opencode");
    let fixture = Fixture::new("opencode-no-entry");
    let original = serde_json::json!({"mcp": {"servers": {"other": {"command": "other"}}}});
    let path = fixture.write_raw(opencode, &original.to_string());
    let before = fs::read(&path).expect("the file can be read");

    let message = uninstall(opencode, SERVER_NAME, &fixture.empty_env())
        .expect("a parseable file without Clio is not an error");
    assert_eq!(message, "nothing to remove");
    assert_eq!(
        fs::read(&path).expect("the file can be read"),
        before,
        "a no-op must not rewrite the file"
    );
    assert!(
        fixture.backup_siblings(opencode).is_empty(),
        "a no-op must not make a backup"
    );
}

#[cfg(unix)]
#[test]
fn antigravity_install_writes_and_verifies_the_mcpservers_entry() {
    let antigravity = record_named("antigravity");
    let mut fixture = Fixture::new("antigravity-install");
    let env = fixture
        .stub_env_vendor_for(antigravity, false)
        .expect("the stub needs a directory that permits exec");
    let message = install(antigravity, SERVER_NAME, &fixture.exe(), &env)
        .expect("the stub wrote the mcpServers entry the read-back accepts");
    assert_eq!(message, "installed");
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&antigravity.add_argv(SERVER_NAME, &fixture.exe()))
    );
}

#[cfg(unix)]
#[test]
fn antigravity_install_reports_already_installed_on_a_duplicate_exit() {
    let antigravity = record_named("antigravity");
    let mut fixture = Fixture::new("antigravity-duplicate");
    fixture.write_config_for(antigravity, &fixture.exe());
    let env = fixture
        .stub_env_vendor_for(antigravity, false)
        .expect("the stub needs a directory that permits exec");
    let message = install(antigravity, SERVER_NAME, &fixture.exe(), &env)
        .expect("an entry already present is a success");
    assert_eq!(message, "already installed");
}

#[cfg(unix)]
#[test]
fn antigravity_uninstall_reports_nothing_to_remove_on_a_non_zero_exit() {
    let antigravity = record_named("antigravity");
    let mut fixture = Fixture::new("antigravity-absent");
    let env = fixture
        .stub_env_for(antigravity, 1)
        .expect("the stub needs a directory that permits exec");
    let message = uninstall(antigravity, SERVER_NAME, &env)
        .expect("a refused remove with no entry is a success");
    assert_eq!(message, "nothing to remove");
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&antigravity.remove_argv(SERVER_NAME))
    );
}

#[cfg(unix)]
#[test]
fn hermes_install_pipes_the_confirmation_and_verifies_the_saved_config() {
    let hermes = record_named("hermes");
    let mut fixture = Fixture::new("hermes-install");
    let env = fixture
        .stub_env_vendor_for(hermes, true)
        .expect("the stub needs a directory that permits exec");
    let message = install(hermes, SERVER_NAME, &fixture.exe(), &env)
        .expect("the stub read the confirmation and saved the config");
    assert_eq!(message, "installed");
    assert_eq!(
        fixture.recorded_stdin(),
        "y\n",
        "the fixed confirmation must reach the vendor"
    );
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&hermes.add_argv(SERVER_NAME, &fixture.exe()))
    );
}

#[cfg(unix)]
#[test]
fn hermes_add_that_exits_zero_without_saving_fails_verification() {
    let hermes = record_named("hermes");
    let mut fixture = Fixture::new("hermes-no-save");
    let env = fixture
        .stub_env_reads_stdin(hermes, 0)
        .expect("the stub needs a directory that permits exec");
    let err = install(hermes, SERVER_NAME, &fixture.exe(), &env)
        .expect_err("exit 0 without a saved config must not claim success");
    assert!(
        matches!(err, DelegateError::VerificationFailed { .. }),
        "{err:?}"
    );
    assert_eq!(
        fixture.recorded_stdin(),
        "y\n",
        "the stub still received the piped confirmation"
    );
}

#[cfg(unix)]
#[test]
fn hermes_uninstall_accepts_a_clean_absent_exit() {
    let hermes = record_named("hermes");
    let mut fixture = Fixture::new("hermes-uninstall");
    let env = fixture
        .stub_env_for(hermes, 0)
        .expect("the stub needs a directory that permits exec");
    let message = uninstall(hermes, SERVER_NAME, &env)
        .expect("a clean exit with no entry is accepted by the mapping");
    assert_eq!(message, "removed");
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&hermes.remove_argv(SERVER_NAME))
    );
}
