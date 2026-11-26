// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin the exclusive creation of
//! a backup: an entry already holding the backup name is refused, whether it is
//! a regular file, a working symlink, or a dangling symlink, and a name with no
//! entry receives the source's bytes and permission bits.
//!
//! ## Owns
//! - The refuse-an-existing-entry tests and the fresh-name success test, all
//!   driving `write_backup` with an explicit target
//!
//! ## Does not own
//! - The behaviour tests of the editor (`agent_harness_write_tests`)
//! - The resolution-before-write tests (`agent_harness_write_toctou_tests`)
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
            "clio-agent-harness-backup-{label}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("scratch directory can be created");
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn write(&self, name: &str, contents: &[u8]) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, contents).expect("fixture can be written");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A source config inside its own directory, and the backup name that source
/// would be copied to. The name is explicit: `backup_file` chooses the stamp,
/// `write_backup` takes the name it is given, so the test writes it out itself.
fn fixtures(scratch: &Scratch) -> (PathBuf, PathBuf) {
    fs::create_dir_all(scratch.path("config")).expect("config directory can be created");
    let source = scratch.write("config/opencode.json", b"{\"mcp\":{\"servers\":{}}}");
    let target = scratch.path("config/opencode.json.clio-backup-1");
    (source, target)
}

/// Every refusal here must be the same classification — an I/O error — so a
/// caller cannot mistake a refused backup for a successful one.
fn assert_refused(result: Result<(), WriteError>) {
    let err = result.expect_err("an entry already at the backup name is refused");
    assert!(
        matches!(err, WriteError::Io { .. }),
        "an existing entry is refused as an I/O error, got {err:?}"
    );
}

/// A link planted at the backup name must not be followed: the file it points
/// at keeps its bytes and the link itself survives, so the copy reaches neither
/// the victim nor the entry.
#[test]
fn a_symlink_at_the_backup_name_is_refused_and_leaves_its_victim_untouched() {
    let scratch = Scratch::new("symlink");
    let (source, target) = fixtures(&scratch);
    let victim = scratch.write("victim.json", b"keep me");
    let victim_before = fs::read(&victim).expect("victim can be read");
    symlink(&victim, &target).expect("a symlink can be planted at the backup name");

    assert_refused(write_backup(&source, &target));

    assert_eq!(
        fs::read(&victim).expect("victim can be read"),
        victim_before,
        "the linked victim must stay byte-identical"
    );
    assert!(
        fs::symlink_metadata(&target)
            .expect("the entry survives")
            .file_type()
            .is_symlink(),
        "the planted entry must still be the symlink, not a regular file"
    );
    assert_eq!(
        fs::read_link(&target).expect("the link can be read"),
        victim,
        "the link must still point where it did"
    );
}

/// A link that resolves to nothing is still an existing entry, so it blocks the
/// backup name: nothing may be created through it and it stays dangling.
#[test]
fn a_dangling_symlink_at_the_backup_name_is_refused() {
    let scratch = Scratch::new("dangling");
    let (source, target) = fixtures(&scratch);
    let absent = scratch.path("never-created.json");
    symlink(&absent, &target).expect("a dangling symlink can be planted");

    assert_refused(write_backup(&source, &target));

    assert!(
        fs::symlink_metadata(&target)
            .expect("the entry survives")
            .file_type()
            .is_symlink(),
        "the dangling entry must still be the symlink"
    );
    assert_eq!(
        fs::read_link(&target).expect("the link can be read"),
        absent,
        "the link must still point at the path that does not exist"
    );
    assert!(
        fs::metadata(&target).is_err(),
        "the link must still resolve to nothing, so nothing was created through it"
    );
}

/// The refusal is exclusivity, not a symlink check: a plain file already at the
/// backup name keeps its own bytes.
#[test]
fn a_regular_file_at_the_backup_name_is_refused() {
    let scratch = Scratch::new("regular");
    let (source, target) = fixtures(&scratch);
    let existing: &[u8] = b"an earlier backup";
    fs::write(&target, existing).expect("an existing entry can be written");

    assert_refused(write_backup(&source, &target));

    assert_eq!(
        fs::read(&target).expect("the existing entry can be read"),
        existing,
        "the existing entry's bytes must not be overwritten"
    );
}

/// A name with no entry is created with the source's bytes and the source's
/// permission bits, which is what the copy this replaced produced.
#[test]
fn a_fresh_backup_name_holds_the_source_bytes_and_mode() {
    let scratch = Scratch::new("fresh");
    let (source, target) = fixtures(&scratch);
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).expect("mode can be set");
    let source_bytes = fs::read(&source).expect("source can be read");

    write_backup(&source, &target).expect("a fresh name is created");

    assert_eq!(
        fs::read(&target).expect("the backup can be read"),
        source_bytes,
        "the backup must hold exactly the source's bytes"
    );
    let mode = |path: &Path| {
        fs::metadata(path)
            .expect("the entry exists")
            .permissions()
            .mode()
            & 0o7777
    };
    assert_eq!(
        mode(&target),
        mode(&source),
        "the backup must carry the source's permission bits"
    );
}
