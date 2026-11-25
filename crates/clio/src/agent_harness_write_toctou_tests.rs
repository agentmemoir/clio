// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin the anchored atomic write
//! of a foreign-config file: one directory descriptor judges and replaces the
//! same entry, and no second path lookup can redirect the edit.
//!
//! ## Owns
//! - Resolved-target write tests: a repointed link, a symlinked final entry,
//!   a new file, and temp-sibling cleanup
//!
//! ## Does not own
//! - The behaviour tests of the editor (`agent_harness_write_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

#![cfg(unix)]

use super::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// One test's own directory, removed when the guard drops. No other test
/// shares it.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "clio-agent-harness-toctou-{label}-{}-{nanos}-{seq}",
            std::process::id()
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

    /// Every `.tmp` sibling in this scratch directory, sorted.
    fn temp_siblings(&self) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(&self.root)
            .expect("scratch can be read")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "tmp"))
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

/// A link that moves after the single resolution must not redirect the write:
/// the resolved target gets the new bytes and the new link target keeps its
/// own. This is the defect the anchored write removes.
#[test]
fn a_resolved_write_does_not_follow_a_repointed_symlink() {
    let scratch = Scratch::new("repointed");
    let real = scratch.write("real.json", r#"{"mcp":{"servers":{}}}"#);
    let victim = scratch.write("victim.json", r#"{"keep":"me"}"#);
    let victim_before = fs::read(&victim).expect("victim can be read");
    let link = scratch.path("opencode.json");
    symlink(&real, &link).expect("a symlink can be made");

    let resolved = effective_path(&link).expect("one link resolves");

    fs::remove_file(&link).expect("the link can be replaced");
    symlink(&victim, &link).expect("the link can be repointed");

    let written = serde_json::json!({"mcp": {"servers": {"written": true}}});
    write_json_atomic_resolved(&resolved, &written).expect("the resolved target is replaced");

    assert_eq!(
        fs::read(&victim).expect("victim can be read"),
        victim_before,
        "the repointed link's target must stay byte-identical"
    );
    let after: Value =
        serde_json::from_str(&fs::read_to_string(&real).expect("target can be read"))
            .expect("the target holds valid JSON");
    assert_eq!(after, written, "the resolved target holds the new content");
    assert!(
        scratch.temp_siblings().is_empty(),
        "a successful write leaves no temp sibling"
    );
}

/// A symlink at the resolved target's final entry is refused rather than
/// followed, and its target keeps its own bytes.
#[test]
fn a_symlink_at_the_target_entry_is_refused() {
    let scratch = Scratch::new("entry-link");
    let victim = scratch.write("victim.json", r#"{"keep":"me"}"#);
    let victim_before = fs::read(&victim).expect("victim can be read");
    let target = scratch.path("real.json");
    symlink(&victim, &target).expect("a symlink can be made");

    let err = write_json_atomic_resolved(&target, &serde_json::json!({"written": true}))
        .expect_err("a symlinked final entry is refused");
    assert!(matches!(err, WriteError::Io { .. }), "got {err:?}");

    assert_eq!(
        fs::read(&victim).expect("victim can be read"),
        victim_before,
        "the symlink target must stay byte-identical"
    );
    assert!(
        fs::symlink_metadata(&target)
            .expect("the entry still exists")
            .file_type()
            .is_symlink(),
        "the refused entry is left in place"
    );
    assert!(
        scratch.temp_siblings().is_empty(),
        "a refused write leaves no temp sibling"
    );
}

/// A new file is created with mode `0o644`, holds the serialized value, and
/// leaves no temp sibling behind.
#[test]
fn a_new_file_is_written_with_the_default_mode() {
    let scratch = Scratch::new("new-file");
    let path = scratch.path("opencode.json");
    let value = serde_json::json!({"mcp": {"servers": {}}});

    write_json_atomic(&path, &value).expect("a new file is written");

    let text = fs::read_to_string(&path).expect("file can be read");
    assert_eq!(
        serde_json::from_str::<Value>(&text).expect("file holds valid JSON"),
        value
    );
    let mode = fs::metadata(&path)
        .expect("file exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644, "a new file gets the default mode");
    assert!(
        scratch.temp_siblings().is_empty(),
        "a successful write leaves no temp sibling"
    );
}
