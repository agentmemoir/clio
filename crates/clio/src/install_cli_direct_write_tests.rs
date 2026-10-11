// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the direct-write path of the `clio install` /
//! `clio uninstall` surfaces at the CLI layer: config resolution through the
//! injected `DirectWriteEnv` seam, exit codes, success and refusal messages,
//! and the file effects each verb leaves behind.
//!
//! ## Owns
//! - The `Scratch` unique temp directory guard and CLI-level assertions over
//!   `run_with` and `direct_write_outcome_text` for the cursor and
//!   github copilot direct-write harnesses
//!
//! ## Does not own
//! - The surface implementation under test (`install_cli`)
//! - The merge, backup, and refusal behavior of the writer itself
//!   (`agent_harness_write_install`), which has its own test matrix
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::tests::{args, fake_exe, never_install, never_uninstall};
use super::*;
use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Nanoseconds plus a process-global counter: two calls never collide.
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{nanos}-{}", DIR_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// One test's own directory, removed when the guard drops.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "clio-install-cli-direct-write-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).expect("scratch directory can be created");
        Self { root }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A Linux-layout environment rooted at the scratch directory, so the
/// resolved config paths land inside the test's own directory.
fn temp_env(home: &Path, xdg_config_home: Option<&Path>) -> DirectWriteEnv {
    DirectWriteEnv::with(
        OsLayout::Linux,
        home.to_path_buf(),
        xdg_config_home.map(Path::to_path_buf),
        None,
        None,
    )
}

/// The cursor config the registry resolves for the scratch environment.
fn cursor_config(scratch: &Scratch) -> DirectWriteConfig {
    agent_harness::resolve("cursor")
        .expect("cursor is supported")
        .direct_write_config(&temp_env(&scratch.root, None))
        .expect("cursor resolves a config under a home")
}

/// Write `text` to the cursor config path of the scratch environment.
fn seed_cursor_file(scratch: &Scratch, text: &str) -> PathBuf {
    let path = cursor_config(scratch).path;
    fs::create_dir_all(path.parent().expect("the config has a parent"))
        .expect("the config parent directory can be created");
    fs::write(&path, text).expect("the seeded config can be written");
    path
}

#[test]
fn install_cursor_against_a_temp_home_creates_the_config_and_exits_0() {
    let scratch = Scratch::new("install-cursor");
    let code = run_with(
        Verb::Install,
        &args(&["--harness", "cursor"]),
        &|| Ok(fake_exe()),
        &|| temp_env(&scratch.root, None),
        &never_install,
        &never_uninstall,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    let config = cursor_config(&scratch);
    let text = fs::read_to_string(&config.path).expect("the created config can be read");
    let root: serde_json::Value = serde_json::from_str(&text).expect("the config is valid JSON");
    let clio = &root[config.top_level_key][super::SERVER_NAME];
    assert_eq!(clio["command"], serde_json::json!(fake_exe()));
    assert_eq!(clio["args"], serde_json::json!(["mcp", "stdio"]));
}

#[test]
fn an_unparseable_config_refuses_the_install_and_leaves_the_file_byte_identical() {
    let scratch = Scratch::new("refuse-install");
    let path = seed_cursor_file(
        &scratch,
        "{\"mcpServers\": {\n  // a human note\n  \"other\": {}\n}}",
    );
    let before = fs::read(&path).expect("the seeded config can be read");
    let code = run_with(
        Verb::Install,
        &args(&["--harness", "cursor"]),
        &|| Ok(fake_exe()),
        &|| temp_env(&scratch.root, None),
        &never_install,
        &never_uninstall,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "an unparseable config is a precondition failure, not a usage error"
    );
    assert_eq!(
        fs::read(&path).expect("the refused config can be read"),
        before,
        "the refused file must stay byte-identical"
    );
}

#[test]
fn the_install_refusal_message_prints_the_manual_snippet() {
    let scratch = Scratch::new("refuse-message");
    seed_cursor_file(&scratch, "{\"mcpServers\": {\"other\": not json}}");
    let config = cursor_config(&scratch);
    let outcome = direct_write_outcome_text(
        Verb::Install,
        agent_harness::resolve("cursor").expect("cursor is supported"),
        &config,
        &agent_harness::server_argv(&fake_exe()),
    );
    let message = outcome.expect_err("an unparseable config must refuse the install");
    assert!(
        message.contains("could not be parsed as JSON")
            && message.contains("add this entry by hand"),
        "{message}"
    );
    assert!(
        message.contains("\"mcpServers\"") && message.contains("\"clio\""),
        "the message must carry the manual entry snippet: {message}"
    );
    assert!(
        message.contains("(offending line: 1)"),
        "the message must name the offending line's number: {message}"
    );
    assert!(
        !message.contains("other"),
        "the message must not copy the file's other entries: {message}"
    );
}

#[test]
fn merging_install_reports_the_path_and_the_backup_and_keeps_foreign_entries() {
    let scratch = Scratch::new("merge-install");
    let path = seed_cursor_file(
        &scratch,
        "{\"mcpServers\": {\"other\": {\"command\": \"x\"}}}",
    );
    let config = cursor_config(&scratch);
    let text = direct_write_outcome_text(
        Verb::Install,
        agent_harness::resolve("cursor").expect("cursor is supported"),
        &config,
        &agent_harness::server_argv(&fake_exe()),
    )
    .expect("a merge into a parseable file succeeds");
    assert!(
        text.contains(&config.path.display().to_string()) && text.contains("backed up to"),
        "{text}"
    );
    let root: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("the merged config can be read"))
            .expect("the merged config is valid JSON");
    assert!(root[config.top_level_key].get("other").is_some());
    assert!(root[config.top_level_key].get(super::SERVER_NAME).is_some());
}

#[test]
fn uninstall_removes_only_clios_entry_and_names_the_backup() {
    let scratch = Scratch::new("uninstall");
    let path = seed_cursor_file(
        &scratch,
        "{\"mcpServers\": {\"other\": {\"command\": \"x\"}, \"clio\": {\"command\": \"clio\"}}}",
    );
    let config = cursor_config(&scratch);
    let text = direct_write_outcome_text(
        Verb::Uninstall,
        agent_harness::resolve("cursor").expect("cursor is supported"),
        &config,
        &agent_harness::server_argv(&fake_exe()),
    )
    .expect("removing a present entry succeeds");
    assert!(
        text.contains("removed clio from")
            && text.contains(&config.path.display().to_string())
            && text.contains("backed up to"),
        "{text}"
    );
    let root: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("the edited config can be read"))
            .expect("the edited config is valid JSON");
    assert!(root[config.top_level_key].get("other").is_some());
    assert!(root[config.top_level_key].get(super::SERVER_NAME).is_none());
}

#[test]
fn uninstall_without_a_clio_entry_is_a_clear_no_op() {
    let scratch = Scratch::new("uninstall-absent");
    let path = seed_cursor_file(
        &scratch,
        "{\"mcpServers\": {\"other\": {\"command\": \"x\"}}}",
    );
    let before = fs::read(&path).expect("the seeded config can be read");
    let config = cursor_config(&scratch);
    let text = direct_write_outcome_text(
        Verb::Uninstall,
        agent_harness::resolve("cursor").expect("cursor is supported"),
        &config,
        &agent_harness::server_argv(&fake_exe()),
    )
    .expect("removing an absent entry succeeds");
    assert!(
        text.contains("nothing to remove") && text.contains(&config.path.display().to_string()),
        "{text}"
    );
    assert_eq!(
        fs::read(&path).expect("the untouched config can be read"),
        before,
        "a no-op uninstall must not rewrite the file"
    );
}

#[test]
fn uninstall_succeeds_when_the_executable_cannot_be_resolved() {
    let scratch = Scratch::new("uninstall-no-exe");
    seed_cursor_file(
        &scratch,
        "{\"mcpServers\": {\"other\": {\"command\": \"x\"}, \"clio\": {\"command\": \"clio\"}}}",
    );
    let config = cursor_config(&scratch);
    let code = run_with(
        Verb::Uninstall,
        &args(&["--harness", "cursor"]),
        &|| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone")),
        &|| temp_env(&scratch.root, None),
        &never_install,
        &never_uninstall,
    );
    assert_eq!(
        code,
        ExitCode::SUCCESS,
        "a valid uninstall must not fail on an unresolvable executable"
    );
    let root: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&config.path).expect("the edited config can be read"),
    )
    .expect("the edited config is valid JSON");
    assert!(
        root[config.top_level_key].get(super::SERVER_NAME).is_none(),
        "clio's entry must be gone: {root}"
    );
    assert!(
        root[config.top_level_key].get("other").is_some(),
        "the foreign entry must survive: {root}"
    );
}

#[test]
fn install_prints_the_copilot_fallback_path_when_the_profile_is_unresolvable() {
    let scratch = Scratch::new("install-copilot-fallback-message");
    // An empty home resolves no VS Code profile and no `~/.copilot`, so the
    // injected `COPILOT_HOME` is the only place the config can land.
    let env = DirectWriteEnv::with(
        OsLayout::Linux,
        PathBuf::new(),
        None,
        None,
        Some(scratch.root.join(".copilot")),
    );
    let record = agent_harness::resolve("github copilot").expect("github copilot is supported");
    let config = record
        .direct_write_config(&env)
        .expect("the unresolvable profile falls back to the Copilot file");
    assert_eq!(
        config.path,
        scratch.root.join(".copilot").join("mcp-config.json"),
        "the unresolvable profile must resolve the fallback file"
    );
    let text = direct_write_outcome_text(
        Verb::Install,
        record,
        &config,
        &agent_harness::server_argv(&fake_exe()),
    )
    .expect("an install into the missing fallback file succeeds");
    assert!(
        text.contains(&config.path.display().to_string()),
        "the message must name the fallback file it wrote: {text}"
    );
}

#[test]
fn install_github_copilot_writes_the_vs_code_profile_file() {
    let scratch = Scratch::new("install-copilot");
    let code = run_with(
        Verb::Install,
        &args(&["--harness", "github copilot"]),
        &|| Ok(fake_exe()),
        &|| temp_env(&scratch.root, Some(&scratch.root)),
        &never_install,
        &never_uninstall,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    let record = agent_harness::resolve("github copilot").expect("github copilot is supported");
    let config = record
        .direct_write_config(&temp_env(&scratch.root, Some(&scratch.root)))
        .expect("copilot resolves a config with an XDG config home");
    let text = fs::read_to_string(&config.path).expect("the created config can be read");
    let root: serde_json::Value = serde_json::from_str(&text).expect("the config is valid JSON");
    let clio = &root[config.top_level_key][super::SERVER_NAME];
    assert_eq!(clio["type"], serde_json::json!("stdio"));
    assert_eq!(clio["command"], serde_json::json!(fake_exe()));
}

#[test]
fn an_empty_home_fails_the_cursor_install_before_any_write() {
    // An empty home resolves no `.cursor` directory, so the record has no
    // config file and the verb must fail as a precondition, not a usage error.
    let code = run_with(
        Verb::Install,
        &args(&["--harness", "cursor"]),
        &|| Ok(fake_exe()),
        &|| DirectWriteEnv::with(OsLayout::Linux, PathBuf::new(), None, None, None),
        &never_install,
        &never_uninstall,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "an unresolvable config is a precondition failure, not a usage error"
    );
}

#[test]
fn a_failing_exe_lookup_fails_the_direct_write_install() {
    let scratch = Scratch::new("install-no-exe");
    let code = run_with(
        Verb::Install,
        &args(&["--harness", "cursor"]),
        &|| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone")),
        &|| temp_env(&scratch.root, None),
        &never_install,
        &never_uninstall,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "an exe-lookup failure is a precondition failure, not a usage error"
    );
}

#[test]
fn an_unparseable_config_refuses_the_uninstall_with_exit_1() {
    let scratch = Scratch::new("refuse-uninstall");
    seed_cursor_file(&scratch, "{\"mcpServers\": {\"other\": not json}}");
    let code = run_with(
        Verb::Uninstall,
        &args(&["--harness", "cursor"]),
        &|| Ok(fake_exe()),
        &|| temp_env(&scratch.root, None),
        &never_install,
        &never_uninstall,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "an unparseable config is a precondition failure, not a usage error"
    );
}

#[test]
fn a_config_that_is_not_a_json_object_refuses_the_install_with_an_io_detail() {
    let scratch = Scratch::new("not-an-object");
    seed_cursor_file(&scratch, "[]");
    let config = cursor_config(&scratch);
    let message = direct_write_outcome_text(
        Verb::Install,
        agent_harness::resolve("cursor").expect("cursor is supported"),
        &config,
        &agent_harness::server_argv(&fake_exe()),
    )
    .expect_err("a parsed non-object config must refuse the install");
    assert!(message.contains("does not hold a JSON object"), "{message}");
    assert!(
        !message.contains("add this entry by hand"),
        "an Io refusal is not a parse refusal and must not carry the snippet: {message}"
    );
}
