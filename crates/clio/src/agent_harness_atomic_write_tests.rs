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

/// Restores a directory's mode when it drops, so a panic cannot leave the
/// scratch directory unreadable.
struct ModeGuard<'a> {
    path: &'a Path,
    mode: u32,
}

impl Drop for ModeGuard<'_> {
    fn drop(&mut self) {
        let _ = fs::set_permissions(self.path, fs::Permissions::from_mode(self.mode));
    }
}

/// A permission refusal is only observable without root, which bypasses the
/// check. Coverage runs as the runner user.
fn running_as_root() -> bool {
    rustix::process::geteuid().is_root()
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

/// The error detail of a refused write, for a message assertion.
fn io_detail(err: WriteError) -> String {
    match err {
        WriteError::Io { detail } => detail,
        WriteError::Unparseable { .. } => panic!("expected an I/O error"),
    }
}

/// A directory at the held entry is refused, and it stays a directory.
#[test]
fn a_directory_at_the_target_entry_is_refused() {
    let scratch = Scratch::new("entry-dir");
    let target = scratch.path("config.json");
    fs::create_dir(&target).expect("a directory can be made at the entry");

    let anchored = AnchoredTarget::open(&target).expect("the parent directory opens once");
    let err = anchored
        .replace(b"{}")
        .expect_err("a directory final entry is refused");

    assert!(
        io_detail(err).contains("not a regular file"),
        "the refusal names the entry kind"
    );
    assert!(target.is_dir(), "the refused entry is left in place");
    assert!(
        temp_siblings_in(&scratch.root).is_empty(),
        "a refused write leaves no temp sibling"
    );
}

/// A FIFO at the held entry is refused before any open can block on it.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn a_fifo_at_the_target_entry_is_refused() {
    let scratch = Scratch::new("entry-fifo");
    let target = scratch.path("config.json");
    let dir = rustix::fs::open(&scratch.root, dir_flags(), rustix::fs::Mode::empty())
        .expect("the scratch directory opens");
    rustix::fs::mkfifoat(&dir, "config.json", rustix::fs::Mode::from_raw_mode(0o644))
        .expect("a fifo can be made at the entry");
    drop(dir);

    let anchored = AnchoredTarget::open(&target).expect("the parent directory opens once");
    let err = anchored
        .replace(b"{}")
        .expect_err("a fifo final entry is refused");

    assert!(io_detail(err).contains("not a regular file"));
    assert!(
        temp_siblings_in(&scratch.root).is_empty(),
        "a refused write leaves no temp sibling"
    );
}

/// An existing regular file keeps its own mode, not the default.
#[test]
fn an_existing_regular_file_keeps_its_mode() {
    let scratch = Scratch::new("existing-mode");
    let target = scratch.write("config.json", "old");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
        .expect("the fixture mode can be narrowed");

    let anchored = AnchoredTarget::open(&target).expect("the parent directory opens once");
    anchored.replace(b"new").expect("the entry is replaced");

    assert_eq!(fs::read(&target).expect("the entry can be read"), b"new");
    let mode = fs::metadata(&target)
        .expect("the entry exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the entry keeps its own mode");
}

/// `remove_temp` deletes an existing sibling and tolerates a missing one.
#[test]
fn remove_temp_deletes_once_and_tolerates_absence() {
    let scratch = Scratch::new("remove-temp");
    let dir = rustix::fs::open(&scratch.root, dir_flags(), rustix::fs::Mode::empty())
        .expect("the scratch directory opens");
    let temp = scratch.write("orphan.tmp", "bytes");
    let name = std::ffi::OsStr::new("orphan.tmp");

    remove_temp(&dir, name);
    assert!(!temp.exists(), "the temp sibling is removed");

    remove_temp(&dir, name);
    assert_eq!(
        fs::read_dir(&scratch.root)
            .expect("the scratch directory can be read")
            .count(),
        0,
        "a missing temp sibling is tolerated"
    );
}

/// A parent directory that does not exist is an open error.
#[test]
fn open_fails_when_the_parent_directory_does_not_exist() {
    let scratch = Scratch::new("open-missing");
    let target = scratch.path("absent").join("config.json");
    let err = AnchoredTarget::open(&target)
        .err()
        .expect("a missing parent is refused");

    assert!(
        io_detail(err).contains("failed to open"),
        "the failure names the open step"
    );
}

/// An entry name over the filesystem limit fails the final-entry inspection.
#[test]
fn an_entry_name_that_is_too_long_is_an_inspect_error() {
    let scratch = Scratch::new("long-name");
    let target = scratch.path(&"x".repeat(300));
    let anchored = AnchoredTarget::open(&target).expect("the parent directory opens once");
    let err = anchored
        .replace(b"{}")
        .expect_err("an over-long entry name cannot be inspected");

    assert!(
        io_detail(err).contains("failed to inspect"),
        "the failure names the inspect step"
    );
}

/// A directory that loses write permission fails temp creation, and the mode
/// is restored whether the assertions pass or not.
#[test]
fn replace_fails_when_the_directory_loses_write_permission() {
    if running_as_root() {
        return;
    }
    let scratch = Scratch::new("read-only-dir");
    let target = scratch.path("config.json");
    let anchored = AnchoredTarget::open(&target).expect("the parent directory opens once");

    fs::set_permissions(&scratch.root, fs::Permissions::from_mode(0o500))
        .expect("the directory can be made read-only");
    let guard = ModeGuard {
        path: &scratch.root,
        mode: 0o755,
    };
    let err = anchored
        .replace(b"{}")
        .expect_err("a read-only directory cannot hold a temp sibling");

    fs::set_permissions(&scratch.root, fs::Permissions::from_mode(0o755))
        .expect("the directory mode is restored");
    drop(guard);
    assert!(
        io_detail(err).contains("failed to create"),
        "the failure names the create step"
    );
    assert!(
        temp_siblings_in(&scratch.root).is_empty(),
        "a failed create leaves no temp sibling"
    );
}

/// A read-only file maps a write failure and returns before the rename.
#[test]
fn write_and_sync_at_maps_a_write_failure() {
    let scratch = Scratch::new("write-fail");
    let target = scratch.write("read-only.json", "old");
    let mut file = fs::File::open(&target).expect("the fixture opens read-only");
    let err = write_and_sync_at(
        &mut file,
        b"new",
        rustix::fs::Mode::from_raw_mode(0o644),
        &target,
    )
    .expect_err("a read-only file cannot be written");

    assert!(
        io_detail(err).contains("failed to write"),
        "the failure names the write step"
    );
}

/// A FIFO accepts the write and rejects the sync, which maps to the sync step.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn write_and_sync_at_maps_a_sync_failure() {
    let scratch = Scratch::new("sync-fail");
    let target = scratch.path("pipe");
    let dir = rustix::fs::open(&scratch.root, dir_flags(), rustix::fs::Mode::empty())
        .expect("the scratch directory opens");
    rustix::fs::mkfifoat(&dir, "pipe", rustix::fs::Mode::from_raw_mode(0o644))
        .expect("a fifo can be made");
    drop(dir);

    let fd = rustix::fs::open(&target, rustix::fs::OFlags::RDWR, rustix::fs::Mode::empty())
        .expect("a fifo opens read-write without blocking");
    let mut file = std::fs::File::from(fd);
    let err = write_and_sync_at(
        &mut file,
        b"bytes",
        rustix::fs::Mode::from_raw_mode(0o644),
        &target,
    )
    .expect_err("a fifo cannot be synced");

    assert!(
        io_detail(err).contains("failed to sync"),
        "the failure names the sync step"
    );
}
