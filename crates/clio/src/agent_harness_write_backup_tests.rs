// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin the exclusive creation of
//! a backup: an entry already holding the backup name is refused, whether it is
//! a regular file, a working symlink, or a dangling symlink; a name with no
//! entry receives the source's bytes and permission bits; and the copy is never
//! readable by anyone the source is not, not even for the instant between the
//! create and the write.
//!
//! ## Owns
//! - The refuse-an-existing-entry tests, driving the create that performs the
//!   refusal with an explicit target, and one that drives the copy `write_backup`
//!   so a plain copy at the copy site cannot stay green
//! - The fresh-name success test, driving `write_backup` with an explicit target
//! - The creation-time mode test, which drives the `create_backup` seam and
//!   reads the mode off the descriptor the create returned, so widening is
//!   observed before any later step could tighten it
//! - The two tests that drive the shipped `backup_file` entry point: the file it
//!   produces, and the state it leaves when a step fails
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

/// The permission bits of an entry, without the file-type bits that an `fstat`
/// mode also carries.
fn mode_bits(path: &Path) -> u32 {
    fs::metadata(path)
        .expect("the entry exists")
        .permissions()
        .mode()
        & 0o7777
}

/// The permission bits of the entry an open descriptor names, so the mode is
/// read through the descriptor the create returned and not by a path lookup
/// that may name a different entry.
fn mode_from(file: &fs::File) -> u32 {
    file.metadata()
        .expect("the open entry can be inspected")
        .permissions()
        .mode()
        & 0o7777
}

/// Every `.clio-backup-` entry in `dir`, sorted. `backup_file` chooses the
/// stamp from the clock, so a test reads whatever name it chose rather than
/// asserting on a stamp of its own.
fn backup_entries(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .expect("the directory can be listed")
        .map(|entry| entry.expect("the entry can be read").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".clio-backup-"))
        })
        .collect();
    found.sort();
    found
}

/// Every refusal here must be the same classification — an I/O error — so a
/// caller cannot mistake a refused backup for a successful one.
fn assert_refused<T: std::fmt::Debug>(result: Result<T, WriteError>) {
    let err = result.expect_err("an entry already at the backup name is refused");
    assert!(
        matches!(err, WriteError::Io { .. }),
        "an existing entry is refused as an I/O error, got {err:?}"
    );
}

/// The permissions the copy asks for: read from the source, which is what the
/// production code does with the handle it already holds.
fn source_permissions(source: &Path) -> fs::Permissions {
    fs::metadata(source)
        .expect("the source can be inspected")
        .permissions()
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

    assert_refused(create_backup(&target, &source_permissions(&source)));

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

    assert_refused(create_backup(&target, &source_permissions(&source)));

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

    assert_refused(create_backup(&target, &source_permissions(&source)));

    assert_eq!(
        fs::read(&target).expect("the existing entry can be read"),
        existing,
        "the existing entry's bytes must not be overwritten"
    );
}

/// A name with no entry is created with the source's bytes and the source's
/// exact permission bits, which is what the copy this replaced produced.
///
/// The source is `0o666` on purpose. The mode is applied at creation and then
/// restored on the open descriptor, so the only way this final mode is `0o666`
/// under a process mask that clears any of those bits — the common `0o022`
/// clears `0o022` of them — is the restoring step. Deleting that step makes the
/// final mode the masked creation mode and this assertion fails. Known limit:
/// under a mask that clears nothing (`0o000`) the creation mode is already
/// `0o666`, so this particular assertion cannot discriminate; the
/// creation-time test below does not depend on the mask at all.
#[test]
fn a_fresh_backup_name_holds_the_source_bytes_and_mode() {
    let scratch = Scratch::new("fresh");
    let (source, target) = fixtures(&scratch);
    fs::set_permissions(&source, fs::Permissions::from_mode(0o666)).expect("mode can be set");
    let source_bytes = fs::read(&source).expect("source can be read");

    write_backup(&source, &target).expect("a fresh name is created");

    assert_eq!(
        fs::read(&target).expect("the backup can be read"),
        source_bytes,
        "the backup must hold exactly the source's bytes"
    );
    assert_eq!(
        mode_bits(&target),
        0o666,
        "the backup must carry the source's exact permission bits, not the \
         creation mode the process mask left"
    );
}

/// The backup is never readable by anyone the source is not, for no interval
/// at all: the mode is one of the bits the create syscall is asked for, so the
/// entry does not exist at any instant with a wider mode than the source.
///
/// The assertion reads the mode off the descriptor the create returned, before
/// anything is written to it, so a mode applied later — even immediately
/// afterwards, as a `set_permissions` right after the create would — cannot
/// satisfy it. Only `O_CREAT` carrying the source's bits can.
#[test]
fn a_backup_is_never_created_wider_than_its_source() {
    let scratch = Scratch::new("widening");
    let (source, target) = fixtures(&scratch);
    let source_mode = 0o600;
    fs::set_permissions(&source, fs::Permissions::from_mode(source_mode)).expect("mode can be set");
    let permissions = fs::metadata(&source)
        .expect("source can be inspected")
        .permissions();

    let created = create_backup(&target, &permissions).expect("a fresh name is created");
    let creation_mode = mode_from(&created);

    assert_eq!(
        creation_mode & !source_mode,
        0,
        "the backup must be created with no permission bit the source \
         {source_mode:#o} lacks; the creation mode was {creation_mode:#o}"
    );
}

/// A source that cannot be read is refused on the shipped path too, and the
/// entry the copy had already created is removed, so the caller that sees the
/// error never finds a sibling holding an unknown prefix of the source's bytes.
///
/// A directory is the source that reaches the recovery branch: `File::open`
/// accepts it and the read then refuses, so the failure lands after the target
/// exists and only the removal keeps it out of the directory. Nothing is left
/// behind, and the source is untouched.
#[test]
fn a_backup_of_a_directory_source_is_refused_and_leaves_no_entry() {
    let scratch = Scratch::new("shipped-refused");
    let (source, _) = fixtures(&scratch);
    let source_before = fs::read(&source).expect("source can be read");
    let config = source
        .parent()
        .expect("the source has a parent")
        .to_path_buf();
    let before = backup_entries(&scratch.root);

    let outcome = backup_file(&config);

    assert!(
        matches!(outcome, Err(WriteError::Io { .. })),
        "a source that cannot be read must surface as an I/O error, got {outcome:?}"
    );
    assert_eq!(
        backup_entries(&scratch.root),
        before,
        "a failed backup must leave no entry behind: the copy is removed, not \
         left half-written where any reader could take it"
    );
    assert_eq!(
        fs::read(&source).expect("source can still be read"),
        source_before,
        "a failed backup must leave the source's bytes unchanged"
    );
    assert_eq!(
        backup_entries(&config),
        Vec::<PathBuf>::new(),
        "no entry may be created in the source's own directory"
    );
}

/// The copy itself must create, never overwrite: an entry already at the target
/// keeps its own bytes.
///
/// This is the one assertion a copy cannot satisfy. `std::fs::copy` reproduces
/// the bytes, the directory, and the mode, so the shipped entry point's own
/// observables are the same either way; but a copy over an existing file
/// destroys it, while the exclusive create refuses and reports an error. The
/// refusal tests above drive the create directly, so this is the layer that
/// pins the copy that sits between them.
#[test]
fn write_backup_refuses_an_existing_entry_instead_of_overwriting_it() {
    let scratch = Scratch::new("write-overwrites");
    let (source, target) = fixtures(&scratch);
    let existing: &[u8] = b"an earlier backup";
    fs::write(&target, existing).expect("an existing entry can be written");

    assert_refused(write_backup(&source, &target));

    assert_eq!(
        fs::read(&target).expect("the existing entry can be read"),
        existing,
        "an existing entry must keep its own bytes, so nothing was copied over it"
    );
}

/// The shipped entry point is what must be right, so one test drives
/// `backup_file` itself rather than handing the copy an explicit target: the
/// stamped name it chooses, the directory it chooses, and the mode it leaves
/// are all the caller's contract. The source is unchanged, and the test removes
/// both files it made.
///
/// What this pins is the wiring and the produced file, not exclusivity: the
/// stamp comes from the clock, so no test can place an entry at the name the
/// call will choose, and `std::fs::copy` also reproduces the bytes, the
/// directory, and the mode. Exclusivity and the creation-time mode are pinned
/// where the target is controllable, by the seam tests above.
#[test]
fn backup_file_returns_a_backup_in_the_source_directory() {
    let scratch = Scratch::new("shipped");
    let (source, _) = fixtures(&scratch);
    fs::set_permissions(&source, fs::Permissions::from_mode(0o666)).expect("mode can be set");
    let source_bytes = fs::read(&source).expect("source can be read");
    let source_mode = mode_bits(&source);

    let backup = backup_file(&source)
        .expect("the source exists, so it is backed up")
        .expect("a source that exists yields a backup path");

    assert!(backup.exists(), "the returned path must exist");
    assert_ne!(backup, source, "the backup must not be the source itself");
    assert_eq!(
        backup.parent(),
        source.parent(),
        "the backup must sit beside the source, not in another directory"
    );
    assert_eq!(
        fs::read(&backup).expect("the backup can be read"),
        source_bytes,
        "the backup must hold exactly the source's bytes"
    );
    assert_eq!(
        mode_bits(&backup),
        source_mode,
        "the backup must carry the source's exact permission bits"
    );
    assert_eq!(
        fs::read(&source).expect("source can still be read"),
        source_bytes,
        "backing up must leave the source's bytes unchanged"
    );
    assert_eq!(
        mode_bits(&source),
        source_mode,
        "backing up must leave the source's mode unchanged"
    );

    fs::remove_file(&backup).expect("the test removes the backup it made");
    assert!(
        fs::symlink_metadata(&backup).is_err(),
        "the produced backup is gone after the test's own cleanup"
    );
    fs::remove_file(&source).expect("the test removes the source it made");
    assert!(
        fs::symlink_metadata(&source).is_err(),
        "the source is gone after the test's own cleanup"
    );
}
