// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the direct-write install and
//! uninstall merge: creation, merging beside foreign entries, parse refusal,
//! reinstall idempotence, uninstall, permission preservation, and the
//! snippet-equals-written-entry contract.
//!
//! ## Owns
//! - The `Scratch` unique temp directory guard, the injected `DirectWriteEnv`
//!   config builders, and one test per behavior of `agent_harness_write_install`
//!
//! ## Does not own
//! - The install implementation under test (`agent_harness_write_install`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::agent_harness::server_argv;
use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Nanoseconds plus a process-global counter: two calls never collide.
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{nanos}-{}", DIR_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// One test's own directory, removed when the guard drops.
pub(crate) struct Scratch {
    root: PathBuf,
}

impl Scratch {
    pub(crate) fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "clio-agent-harness-write-install-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).expect("scratch directory can be created");
        Self { root }
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Every entry of `dir`, sorted; `read_dir` order is never asserted.
pub(crate) fn siblings(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .expect("directory can be read")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

pub(crate) fn backup_siblings(dir: &Path) -> Vec<PathBuf> {
    siblings(dir)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".clio-backup-"))
        })
        .collect()
}

pub(crate) fn config(name: &str, env: &DirectWriteEnv) -> DirectWriteConfig {
    crate::agent_harness::resolve(name)
        .expect("the harness is supported")
        .direct_write_config(env)
        .expect("the harness resolves a direct-write config")
}

/// Cursor's config at `<home>/.cursor/mcp.json`.
pub(crate) fn cursor_config(scratch: &Scratch) -> DirectWriteConfig {
    config(
        "cursor",
        &DirectWriteEnv::with(OsLayout::Linux, scratch.root.clone(), None, None, None),
    )
}

/// GitHub Copilot's VS Code profile file under the injected XDG root.
pub(crate) fn copilot_profile_config(scratch: &Scratch) -> DirectWriteConfig {
    config(
        "github copilot",
        &DirectWriteEnv::with(
            OsLayout::Linux,
            scratch.root.clone(),
            Some(scratch.root.join("xdg")),
            None,
            None,
        ),
    )
}

/// GitHub Copilot's fallback file under the injected Copilot directory,
/// reached because the empty home resolves no VS Code profile.
fn copilot_fallback_config(scratch: &Scratch) -> DirectWriteConfig {
    config(
        "github copilot",
        &DirectWriteEnv::with(
            OsLayout::Linux,
            PathBuf::new(),
            None,
            None,
            Some(scratch.root.join(".copilot")),
        ),
    )
}

pub(crate) fn argv(scratch: &Scratch, exe: &str) -> Vec<OsString> {
    server_argv(&scratch.path(exe))
}

pub(crate) fn exe_string(scratch: &Scratch, exe: &str) -> String {
    scratch.path(exe).to_string_lossy().into_owned()
}

pub(crate) fn parsed(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("config can be read"))
        .expect("the config is valid JSON")
}

/// Write a fixture config, creating its parent directory first: a real
/// config directory does not exist until Clio or the harness creates it.
pub(crate) fn write_fixture(config: &DirectWriteConfig, contents: impl AsRef<[u8]>) {
    fs::create_dir_all(config.path.parent().expect("config has a parent"))
        .expect("config directory can be created");
    fs::write(&config.path, contents).expect("fixture can be written");
}

/// Reinstall replaces only Clio's entry, keeps the foreign entry, backs up.
fn assert_reinstall_replaces_only_cli_entry(scratch: &Scratch, config: &DirectWriteConfig) {
    let foreign = serde_json::json!({"command": "other"});
    let original = serde_json::json!({ (config.top_level_key): {"clio": {"command": "old"}, "other": foreign} });
    write_fixture(config, original.to_string());

    install_server_from_file(config, "clio", &argv(scratch, "clio-bin-a"))
        .expect("the first install succeeds");
    let outcome = install_server_from_file(config, "clio", &argv(scratch, "clio-bin-b"))
        .expect("the reinstall succeeds");

    assert!(matches!(outcome, InstallOutcome::Merged { .. }));
    let after = parsed(&config.path);
    assert_eq!(
        after[config.top_level_key]["clio"]["command"],
        Value::String(exe_string(scratch, "clio-bin-b")),
        "the reinstall wrote the updated command"
    );
    assert_eq!(
        after[config.top_level_key]["other"], foreign,
        "the foreign entry must not change"
    );
    let dir = config.path.parent().expect("config has a parent");
    assert!(
        !backup_siblings(dir).is_empty(),
        "a backup was made before the rewrite"
    );
}

/// Uninstall removes only Clio's entry and keeps the foreign entry.
fn assert_uninstall_removes_only_cli_entry(scratch: &Scratch, config: &DirectWriteConfig) {
    let foreign = serde_json::json!({"command": "other"});
    let original = serde_json::json!({ (config.top_level_key): {"clio": {"command": "clio"}, "other": foreign} });
    write_fixture(config, original.to_string());

    let outcome = remove_server_from_direct_write(config, "clio", &argv(scratch, "clio-bin"))
        .expect("the uninstall succeeds");

    assert!(matches!(outcome, RemoveOutcome::Removed { .. }));
    let after = parsed(&config.path);
    assert_eq!(after[config.top_level_key]["other"], foreign);
    assert!(after[config.top_level_key].get("clio").is_none());
}

/// Uninstall without a Clio entry is a no-op: `Absent`, no write, no backup.
fn assert_uninstall_without_entry_is_a_noop(scratch: &Scratch, config: &DirectWriteConfig) {
    let original = serde_json::json!({ (config.top_level_key): {"other": {"command": "other"}} });
    write_fixture(config, original.to_string());
    let before = fs::read(&config.path).expect("file can be read");

    let outcome = remove_server_from_direct_write(config, "clio", &argv(scratch, "clio-bin"))
        .expect("a parseable file is not an error");

    assert_eq!(outcome, RemoveOutcome::Absent);
    assert_eq!(
        fs::read(&config.path).expect("file can be read"),
        before,
        "the file must not change"
    );
    let dir = config.path.parent().expect("config has a parent");
    assert!(backup_siblings(dir).is_empty(), "no backup should be made");
}

#[test]
fn cursor_missing_file_is_created_with_only_cli_entry() {
    let scratch = Scratch::new("cursor-create");
    let config = cursor_config(&scratch);

    let outcome =
        install_server_from_file(&config, "clio", &argv(&scratch, "clio-bin")).expect("install");

    assert_eq!(outcome, InstallOutcome::Created);
    let after = parsed(&config.path);
    assert_eq!(
        after["mcpServers"]["clio"],
        serde_json::json!({
            "command": exe_string(&scratch, "clio-bin"), "args": ["mcp", "stdio"],
        }),
        "the created entry carries command and args, no type field: {after}"
    );
}

#[test]
fn cursor_merge_keeps_foreign_entries_and_unknown_keys() {
    let scratch = Scratch::new("cursor-merge");
    let config = cursor_config(&scratch);
    let foreign = serde_json::json!({"command": "other"});
    let original =
        serde_json::json!({"mcpServers": {"other": foreign}, "editor": {"telemetry": true}});
    write_fixture(&config, original.to_string());

    let outcome =
        install_server_from_file(&config, "clio", &argv(&scratch, "clio-bin")).expect("install");

    assert!(matches!(outcome, InstallOutcome::Merged { .. }));
    let after = parsed(&config.path);
    assert_eq!(after["mcpServers"]["other"], foreign, "foreign entry kept");
    assert_eq!(after["editor"], original["editor"], "unknown key kept");
    assert!(after["mcpServers"].get("clio").is_some(), "clio added");
    let dir = config.path.parent().expect("config has a parent");
    assert!(
        !backup_siblings(dir).is_empty(),
        "a backup was made before the rewrite"
    );
}

#[test]
fn cursor_reinstall_replaces_only_cli_entry() {
    let scratch = Scratch::new("cursor-reinstall");
    assert_reinstall_replaces_only_cli_entry(&scratch, &cursor_config(&scratch));
}

#[test]
fn cursor_uninstall_removes_only_cli_entry() {
    let scratch = Scratch::new("cursor-uninstall");
    assert_uninstall_removes_only_cli_entry(&scratch, &cursor_config(&scratch));
}

#[test]
fn cursor_uninstall_without_entry_is_a_noop() {
    let scratch = Scratch::new("cursor-noop");
    assert_uninstall_without_entry_is_a_noop(&scratch, &cursor_config(&scratch));
}

#[test]
fn copilot_profile_missing_file_is_created_with_stdio_entry() {
    let scratch = Scratch::new("copilot-create");
    let config = copilot_profile_config(&scratch);

    let outcome =
        install_server_from_file(&config, "clio", &argv(&scratch, "clio-bin")).expect("install");

    assert_eq!(outcome, InstallOutcome::Created);
    let after = parsed(&config.path);
    assert_eq!(
        after["servers"]["clio"],
        serde_json::json!({
            "type": "stdio",
            "command": exe_string(&scratch, "clio-bin"),
            "args": ["mcp", "stdio"],
        }),
        "the created entry carries the stdio type: {after}"
    );
}

#[test]
fn copilot_profile_merge_keeps_foreign_entries_and_inputs() {
    let scratch = Scratch::new("copilot-merge");
    let config = copilot_profile_config(&scratch);
    let foreign = serde_json::json!({"type": "stdio", "command": "other", "args": ["serve"]});
    let inputs = serde_json::json!([{"type": "promptString", "id": "clio-version"}]);
    let original = serde_json::json!({"servers": {"other": foreign}, "inputs": inputs});
    write_fixture(&config, original.to_string());

    let outcome =
        install_server_from_file(&config, "clio", &argv(&scratch, "clio-bin")).expect("install");

    assert!(matches!(outcome, InstallOutcome::Merged { .. }));
    let after = parsed(&config.path);
    assert_eq!(after["servers"]["other"], foreign, "foreign entry kept");
    assert_eq!(after["inputs"], inputs, "the inputs array survived");
}

#[test]
fn copilot_profile_reinstall_replaces_only_cli_entry() {
    let scratch = Scratch::new("copilot-reinstall");
    assert_reinstall_replaces_only_cli_entry(&scratch, &copilot_profile_config(&scratch));
}

#[test]
fn copilot_profile_uninstall_removes_only_cli_entry() {
    let scratch = Scratch::new("copilot-uninstall");
    assert_uninstall_removes_only_cli_entry(&scratch, &copilot_profile_config(&scratch));
}

#[test]
fn copilot_profile_uninstall_without_entry_is_a_noop() {
    let scratch = Scratch::new("copilot-noop");
    assert_uninstall_without_entry_is_a_noop(&scratch, &copilot_profile_config(&scratch));
}

#[test]
fn copilot_fallback_uninstall_removes_only_cli_entry() {
    let scratch = Scratch::new("copilot-fallback");
    let config = copilot_fallback_config(&scratch);
    let foreign = serde_json::json!({"type": "local", "command": "other"});
    let original = serde_json::json!({"mcpServers": {
        "clio": {"type": "local", "command": "clio", "args": ["mcp", "stdio"]},
        "other": foreign,
    }});
    write_fixture(&config, original.to_string());
    let before = parsed(&config.path);
    assert_eq!(
        before["mcpServers"]["clio"]["type"], "local",
        "the fallback file pins the local entry type"
    );

    let outcome = remove_server_from_direct_write(&config, "clio", &argv(&scratch, "clio-bin"))
        .expect("the uninstall succeeds");

    assert!(matches!(outcome, RemoveOutcome::Removed { .. }));
    let after = parsed(&config.path);
    assert_eq!(after["mcpServers"]["other"], foreign);
    assert!(after["mcpServers"].get("clio").is_none());
}

/// An install whose profile is unresolvable writes the Copilot fallback
/// file, the same file the fallback uninstall cleans up.
#[test]
fn copilot_fallback_install_writes_the_fallback_file() {
    let scratch = Scratch::new("copilot-fallback-install");
    let config = copilot_fallback_config(&scratch);

    let outcome =
        install_server_from_file(&config, "clio", &argv(&scratch, "clio-bin")).expect("install");

    assert_eq!(outcome, InstallOutcome::Created);
    let after = parsed(&config.path);
    assert_eq!(
        after["mcpServers"]["clio"],
        serde_json::json!({
            "type": "local",
            "command": exe_string(&scratch, "clio-bin"),
            "args": ["mcp", "stdio"],
        }),
        "the install through the unresolvable profile wrote the fallback file: {after}"
    );
}

#[test]
fn snippet_entry_equals_installed_entry() {
    let scratch = Scratch::new("contract");
    let config = cursor_config(&scratch);
    let argv = argv(&scratch, "clio-bin");

    install_server_from_file(&config, "clio", &argv).expect("install");
    let installed = parsed(&config.path)["mcpServers"]["clio"].clone();
    let snippet = direct_write_entry_snippet(&config, "clio", &argv);
    let fragment: Value = serde_json::from_str(&snippet).expect("the snippet is valid JSON");

    assert_eq!(
        fragment["mcpServers"]["clio"], installed,
        "the printed snippet equals what install writes"
    );
}

/// An atomic rewrite preserves the file's permission bits.
#[cfg(unix)]
fn assert_replaced_file_keeps_its_permission_bits(scratch: &Scratch, config: &DirectWriteConfig) {
    let original = serde_json::json!({ (config.top_level_key): {} });
    write_fixture(config, original.to_string());
    fs::set_permissions(&config.path, fs::Permissions::from_mode(0o600)).expect("mode can be set");

    install_server_from_file(config, "clio", &argv(scratch, "clio-bin")).expect("install");

    let mode = fs::metadata(&config.path)
        .expect("file exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the permission bits must be preserved");
}

#[cfg(unix)]
#[test]
fn replaced_file_keeps_its_permission_bits() {
    let scratch = Scratch::new("cursor-mode");
    assert_replaced_file_keeps_its_permission_bits(&scratch, &cursor_config(&scratch));
}

#[cfg(unix)]
#[test]
fn copilot_profile_replaced_file_keeps_its_permission_bits() {
    let scratch = Scratch::new("copilot-mode");
    assert_replaced_file_keeps_its_permission_bits(&scratch, &copilot_profile_config(&scratch));
}
