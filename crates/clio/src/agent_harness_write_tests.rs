// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the foreign-config JSON editor:
//! backup, parse-refusal, remove-only-ours, absent-file behavior, and
//! permission preservation.
//!
//! ## Owns
//! - The `Scratch` unique temp directory guard and its fixture writers
//! - One test per behavior of `agent_harness_write`
//!
//! ## Does not own
//! - The editor implementation under test (`agent_harness_write`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const V2: &[&str] = &["mcp", "servers"];
const PREFIXES: &[&[&str]] = &[V2];

/// Nanoseconds plus a process-global counter: two calls can never get the same
/// suffix, even when they read the clock in the same nanosecond.
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{nanos}-{}", DIR_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// One test's own directory, removed when the guard drops. No other test
/// shares it.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "clio-agent-harness-write-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).expect("scratch directory can be created");
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, contents).expect("fixture can be written");
        path
    }

    /// Every `.clio-backup-` sibling in this scratch directory, sorted.
    fn backup_siblings(&self) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(&self.root)
            .expect("scratch can be read")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".clio-backup-"))
            })
            .collect();
        found.sort();
        found
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn missing_file_is_absent_and_creates_nothing() {
    let scratch = Scratch::new("missing");
    let path = scratch.path("opencode.json");

    let outcome = remove_server_from_file(&path, PREFIXES, "clio", "clio mcp stdio")
        .expect("a missing file is not an error");

    assert_eq!(outcome, RemoveOutcome::Absent);
    assert!(!path.exists(), "no file should be created");
}

#[test]
fn removal_preserves_foreign_entries_structurally() {
    let scratch = Scratch::new("foreign");
    let foreign = serde_json::json!({"type": "stdio", "command": "other", "args": ["serve"]});
    let original = serde_json::json!({
        "mcp": {"servers": {
            "clio": {"type": "stdio", "command": "clio", "args": ["mcp", "stdio"]},
            "other": foreign,
        }}
    });
    let path = scratch.write("opencode.json", &original.to_string());

    let outcome = remove_server_from_file(&path, PREFIXES, "clio", "clio mcp stdio")
        .expect("a parseable file is edited");

    assert!(matches!(outcome, RemoveOutcome::Removed { .. }));
    // The whole file is re-serialized, so key order and whitespace may change.
    // Foreign entries survive by parsed value, not byte-identical: the foreign
    // entry's value and its presence must survive the round trip.
    let after: Value = serde_json::from_str(&fs::read_to_string(&path).expect("file can be read"))
        .expect("the edited file is valid JSON");
    assert_eq!(
        after["mcp"]["servers"]["other"], original["mcp"]["servers"]["other"],
        "the foreign entry's parsed value must not change"
    );
    assert!(
        after["mcp"]["servers"].get("clio").is_none(),
        "the clio server must be gone"
    );
}

#[test]
fn unparseable_file_is_refused_byte_identical() {
    let scratch = Scratch::new("unparseable");
    // A comment makes this OpenCode-shaped text invalid JSON; such a file is a
    // normal case that must be refused, not rewritten.
    let bytes = b"{ // comment\n\"mcp\": { \"servers\": {} }\n}\n";
    let path = scratch.path("opencode.json");
    fs::write(&path, bytes).expect("fixture can be written");

    let err = remove_server_from_file(&path, PREFIXES, "clio", "clio mcp stdio")
        .expect_err("an unparseable file is refused");

    match err {
        WriteError::Unparseable {
            path: err_path,
            snippet,
        } => {
            assert_eq!(err_path, path);
            assert!(
                snippet.contains("clio mcp stdio"),
                "the caller's snippet is carried: {snippet}"
            );
        }
        other @ WriteError::Io { .. } => panic!("expected Unparseable, got {other:?}"),
    }
    assert_eq!(
        fs::read(&path).expect("file can be read"),
        bytes,
        "the bytes must not change"
    );
}

#[test]
fn unparseable_refusal_combines_the_caller_snippet_and_the_offending_line() {
    let scratch = Scratch::new("unparseable-detail");
    let bytes = b"{ // comment\n\"mcp\": { \"servers\": {} }\n}\n";
    let path = scratch.path("opencode.json");
    fs::write(&path, bytes).expect("fixture can be written");

    let err = remove_server_from_file(&path, PREFIXES, "clio", "clio mcp stdio")
        .expect_err("an unparseable file is refused");
    let WriteError::Unparseable { snippet, .. } = err else {
        panic!("expected Unparseable, got {err:?}");
    };
    assert!(
        snippet.contains("clio mcp stdio") && snippet.contains("(offending line: 1)"),
        "the refusal keeps the manual step and the offending line's number: {snippet}"
    );
    assert!(
        !snippet.contains("// comment"),
        "the refusal must not copy the line's bytes out of the user's config: {snippet}"
    );
    assert_eq!(
        fs::read(&path).expect("file can be read"),
        bytes,
        "a refused file must stay byte-unchanged"
    );
}

#[test]
fn no_clio_entry_is_absent_without_a_backup() {
    let scratch = Scratch::new("no-entry");
    let original = serde_json::json!({"mcp": {"servers": {"other": {"command": "other"}}}});
    let path = scratch.write("opencode.json", &original.to_string());
    let before = fs::read(&path).expect("file can be read");

    let outcome = remove_server_from_file(&path, PREFIXES, "clio", "clio mcp stdio")
        .expect("a parseable file is not an error");

    assert_eq!(outcome, RemoveOutcome::Absent);
    assert!(
        scratch.backup_siblings().is_empty(),
        "no backup should be made"
    );
    assert_eq!(
        fs::read(&path).expect("file can be read"),
        before,
        "the file must not change"
    );
}

#[test]
fn a_removal_writes_a_backup_sibling() {
    let scratch = Scratch::new("backup");
    let original = serde_json::json!({"mcp": {"servers": {"clio": {"command": "clio"}}}});
    let path = scratch.write("opencode.json", &original.to_string());
    let before = fs::read(&path).expect("file can be read");

    let outcome = remove_server_from_file(&path, PREFIXES, "clio", "clio mcp stdio")
        .expect("a parseable file is edited");

    let RemoveOutcome::Removed { backup } = outcome else {
        panic!("expected Removed, got {outcome:?}");
    };
    let backups = scratch.backup_siblings();
    assert_eq!(
        backups,
        vec![backup.clone()],
        "the outcome names the backup that was made"
    );
    assert_eq!(
        fs::read(&backup).expect("backup can be read"),
        before,
        "the backup holds the original bytes"
    );
}

#[test]
fn a_v1_entry_is_removed_from_its_fallback_prefix() {
    let scratch = Scratch::new("fallback");
    let original = serde_json::json!({"mcp": {"clio": {"command": "clio"}, "servers": {}}});
    let path = scratch.write("opencode.json", &original.to_string());

    let outcome = remove_server_from_file(
        &path,
        &[&["mcp", "servers"], &["mcp"]],
        "clio",
        "clio mcp stdio",
    )
    .expect("a parseable file is edited");

    assert!(matches!(outcome, RemoveOutcome::Removed { .. }));
    let after: Value = serde_json::from_str(&fs::read_to_string(&path).expect("file can be read"))
        .expect("the edited file is valid JSON");
    assert!(
        after["mcp"].get("clio").is_none(),
        "the version-1 entry must be gone"
    );
}

#[cfg(unix)]
#[test]
fn a_write_keeps_the_file_permission_bits() {
    let scratch = Scratch::new("mode");
    let original = serde_json::json!({"mcp": {"servers": {"clio": {"command": "clio"}}}});
    let path = scratch.write("opencode.json", &original.to_string());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("mode can be set");

    let outcome = remove_server_from_file(&path, PREFIXES, "clio", "clio mcp stdio")
        .expect("a parseable file is edited");

    assert!(matches!(outcome, RemoveOutcome::Removed { .. }));
    let mode = fs::metadata(&path)
        .expect("file exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o640, "the permission bits must be preserved");
}

#[test]
fn a_config_holding_the_server_under_two_prefixes_is_fully_cleared() {
    let scratch = Scratch::new("dual");
    let original = serde_json::json!({"mcp": {
        "servers": {"clio": {"command": "clio"}},
        "clio": {"command": "clio"},
    }});
    let path = scratch.write("opencode.json", &original.to_string());

    let outcome = remove_server_from_file(
        &path,
        &[&["mcp", "servers"], &["mcp"]],
        "clio",
        "clio mcp stdio",
    )
    .expect("a parseable file is edited");

    assert!(matches!(outcome, RemoveOutcome::Removed { .. }));
    let after: Value = serde_json::from_str(&fs::read_to_string(&path).expect("file can be read"))
        .expect("the edited file is valid JSON");
    assert!(
        after["mcp"]["servers"].get("clio").is_none() && after["mcp"].get("clio").is_none(),
        "no prefix may keep a leftover copy: {after}"
    );
}

#[test]
fn a_removal_leaves_no_temp_sibling() {
    let scratch = Scratch::new("temp-clean");
    let original = serde_json::json!({"mcp": {"servers": {"clio": {"command": "clio"}}}});
    let path = scratch.write("opencode.json", &original.to_string());

    remove_server_from_file(&path, PREFIXES, "clio", "clio mcp stdio")
        .expect("a parseable file is edited");

    let leftovers: Vec<String> = fs::read_dir(&scratch.root)
        .expect("scratch can be read")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tmp"))
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    assert!(
        leftovers.is_empty(),
        "the temp file was renamed: {leftovers:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_config_keeps_its_link() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("symlink");
    let foreign = serde_json::json!({"command": "other"});
    let original = serde_json::json!({
        "mcp": {"servers": {
            "clio": {"command": "clio"},
            "other": foreign,
        }}
    });
    let target = scratch.write("real.json", &original.to_string());
    let before = fs::read(&target).expect("target can be read");
    let link = scratch.path("opencode.json");
    symlink(&target, &link).expect("a symlink can be made");

    let outcome = remove_server_from_file(&link, PREFIXES, "clio", "clio mcp stdio")
        .expect("a symlinked file is edited through its target");

    assert!(matches!(outcome, RemoveOutcome::Removed { .. }));
    assert!(
        fs::symlink_metadata(&link)
            .expect("link exists")
            .file_type()
            .is_symlink(),
        "the link itself must survive the edit"
    );
    let after: Value = serde_json::from_str(&fs::read_to_string(&link).expect("link can be read"))
        .expect("the edited target is valid JSON");
    assert_eq!(
        after["mcp"]["servers"]["other"], original["mcp"]["servers"]["other"],
        "the foreign entry's parsed value must not change"
    );
    assert!(
        after["mcp"]["servers"].get("clio").is_none(),
        "the clio server must be gone from the target"
    );
    assert!(
        scratch.backup_siblings().len() == 1
            && fs::read(&scratch.backup_siblings()[0]).expect("backup can be read") == before,
        "the backup holds the target's original bytes"
    );
}

#[test]
fn backing_up_a_missing_file_creates_nothing() {
    let scratch = Scratch::new("backup-missing");
    assert_eq!(
        backup_file(&scratch.path("gone.json")).expect("a missing file is not an error"),
        None,
        "no file means no backup"
    );
}

#[test]
fn a_prefix_that_resolves_to_nothing_is_skipped() {
    let mut value = serde_json::json!({"mcp": {"clio": {"command": "clio"}}});
    assert!(remove_server_entry(
        &mut value,
        &[&["mcp", "servers"], &["mcp"]],
        "clio"
    ));
    assert!(
        value["mcp"].get("clio").is_none(),
        "the dangling prefix is skipped and the fallback prefix still clears the entry"
    );
}

#[test]
fn a_config_that_is_a_directory_is_refused_as_an_io_error() {
    let scratch = Scratch::new("dir-config");
    let err = remove_server_from_file(&scratch.root, PREFIXES, "clio", "clio mcp stdio")
        .expect_err("a directory is not a readable config file");
    assert!(matches!(err, WriteError::Io { .. }), "got {err:?}");
}

#[test]
fn a_write_beside_a_non_directory_is_refused() {
    let scratch = Scratch::new("not-a-dir");
    let blocker = scratch.write("blocker.json", "{}");
    let err = write_json_atomic(&blocker.join("config.json"), &serde_json::json!({}))
        .expect_err("a temp file cannot be created under a plain file");
    assert!(matches!(err, WriteError::Io { .. }), "got {err:?}");
}

#[cfg(unix)]
#[test]
fn a_symlink_loop_is_refused_after_eight_hops() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("symlink-loop");
    let a = scratch.path("a.json");
    let b = scratch.path("b.json");
    symlink(&b, &a).expect("the first link can be made");
    symlink(&a, &b).expect("the second link can be made");

    let err = remove_server_from_file(&a, PREFIXES, "clio", "clio mcp stdio")
        .expect_err("a symlink loop cannot be resolved");
    let WriteError::Io { detail } = err else {
        panic!("expected Io, got {err:?}");
    };
    assert!(
        detail.contains("too many symlink levels"),
        "the loop is named as the cause: {detail}"
    );
}
