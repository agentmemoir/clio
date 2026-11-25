// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the anchored atomic write: the
//! descriptor opened once decides where the bytes land, a symlinked final
//! entry is refused, and a new file gets the default mode.
//!
//! ## Owns
//! - The `Scratch` unique temp directory guard and its fixture writers
//! - One test per behaviour of `agent_harness_atomic_write`
//!
//! ## Does not own
//! - The atomic write implementation under test (`agent_harness_atomic_write`)
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
            "clio-agent-harness-atomic-{label}-{}-{nanos}-{seq}",
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
}

/// Every `.tmp` sibling in `dir`, sorted.
fn temp_siblings_in(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .expect("directory can be read")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tmp"))
        .collect();
    found.sort();
    found
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The descriptor, not the path, decides where the bytes land. The opened
/// directory is renamed away and a decoy is placed at the old path, so a
/// lookup made after `open` writes to the decoy and fails this test.
#[test]
fn a_write_lands_in_the_opened_directory_not_a_decoy_at_the_path() {
    let scratch = Scratch::new("descriptor");
    let live = scratch.path("live");
    fs::create_dir(&live).expect("target directory can be created");
    let target = live.join("opencode.json");
    fs::write(&target, r#"{"mcp":{"servers":{}}}"#).expect("fixture can be written");

    let anchored = AnchoredTarget::open(&target).expect("the parent directory opens once");

    let moved = scratch.path("moved");
    fs::rename(&live, &moved).expect("the opened directory can be moved");
    fs::create_dir(&live).expect("a decoy can be created at the old path");
    let decoy = live.join("opencode.json");
    fs::write(&decoy, b"decoy").expect("decoy fixture can be written");

    let written = serde_json::json!({"mcp": {"servers": {}}});
    anchored
        .replace(written.to_string().as_bytes())
        .expect("the held entry is replaced");

    let landed = moved.join("opencode.json");
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&landed).expect("the moved entry can be read"))
            .expect("the moved entry holds valid JSON");
    assert_eq!(after, written, "the bytes landed in the opened directory");
    assert_eq!(
        fs::read(&decoy).expect("decoy can be read"),
        b"decoy",
        "the decoy at the old path is untouched"
    );
    assert!(
        temp_siblings_in(&moved).is_empty(),
        "a successful write leaves no temp sibling in the opened directory"
    );
    assert_eq!(
        fs::read_dir(&live)
            .expect("decoy directory can be read")
            .count(),
        1,
        "the decoy directory holds only its own file"
    );
}

/// A symlink at the held entry is refused rather than followed, and its target
/// keeps its own bytes.
#[test]
fn a_symlink_at_the_target_entry_is_refused() {
    let scratch = Scratch::new("entry-link");
    let victim = scratch.write("victim.json", r#"{"keep":"me"}"#);
    let victim_before = fs::read(&victim).expect("victim can be read");
    let target = scratch.path("real.json");
    symlink(&victim, &target).expect("a symlink can be made");

    let anchored = AnchoredTarget::open(&target).expect("the parent directory opens once");
    let err = anchored
        .replace(b"{}")
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
        temp_siblings_in(&scratch.root).is_empty(),
        "a refused write leaves no temp sibling"
    );
}

/// A new file is created with mode `0o644`, holds the bytes, and leaves no
/// temp sibling behind.
#[test]
fn a_new_file_is_written_with_the_default_mode() {
    let scratch = Scratch::new("new-file");
    let path = scratch.path("opencode.json");
    let value = serde_json::json!({"mcp": {"servers": {}}});

    let anchored = AnchoredTarget::open(&path).expect("the parent directory opens once");
    anchored
        .replace(value.to_string().as_bytes())
        .expect("a new file is written");

    let text = fs::read_to_string(&path).expect("file can be read");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).expect("file holds valid JSON"),
        value
    );
    let mode = fs::metadata(&path)
        .expect("file exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644, "a new file gets the default mode");
    assert!(
        temp_siblings_in(&scratch.root).is_empty(),
        "a successful write leaves no temp sibling"
    );
}
