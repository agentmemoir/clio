// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the coverage of export-destination
//! confinement: what it accepts inside the export root and every shape it
//! refuses.
//!
//! ## Owns
//! - The scratch-directory fixture and the confinement assertions
//!
//! ## Does not own
//! - The confinement rules themselves (see `export_destination`)
//! - The MCP tools that call it (see `portability_tools_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Every test owns one scratch directory under the OS temp directory and
//! removes it on drop. No test mutates the process environment or the process
//! working directory.
//!
//! Every refusal test also asserts that the export root was not created, except
//! where the root is deliberately pre-created to reach another branch: a refused
//! destination must leave no trace on the filesystem.

use super::{NO_PARENT, OUTSIDE, confine_export_destination};
use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Scratch directories so far, so no two tests share a path.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One per-test scratch directory, removed when the guard drops.
///
/// `tempfile` is not a workspace dependency, so this is the repo's own fixture
/// pattern: a unique path under the OS temp directory with explicit cleanup.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "am-mcp-export-destination-{tag}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The refusal of a destination a trusted root must reject as the caller's
/// fault, with its message for further inspection.
fn refusal(root: &Path, destination: &str) -> String {
    let err = refused(root, true, destination);
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{}", err.message);
    err.message
}

/// The refusal of a destination that must not be blamed on the caller.
fn internal_refusal(root: &Path, destination: &str) -> String {
    let err = refused(root, true, destination);
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    err.message
}

/// The error itself, for the tests that vary trust rather than the path.
fn refused(root: &Path, trusted: bool, destination: &str) -> AmError {
    confine_export_destination(root, trusted, destination)
        .expect_err("this destination must be refused")
}

/// Confinement validates; it does not rewrite. The returned path is the
/// caller's own absolute destination, so its identity survives the check even
/// where the parent is reached through a symlink.
#[test]
fn destination_inside_the_root_is_accepted_as_given() {
    let scratch = Scratch::new("inside");
    let root = scratch.path().join("root");
    std::fs::create_dir_all(root.join("nested")).expect("root");
    let destination = root.join("./nested/bundle.json");
    let confined = confine_export_destination(&root, true, &destination.to_string_lossy())
        .expect("a destination inside the root is accepted");
    assert!(confined.is_absolute(), "{}", confined.display());
    assert_eq!(confined, destination);
    assert_eq!(confined, root.join("nested").join("bundle.json"));
}

/// The refusal names neither the root nor the destination, so a caller cannot
/// read the deployment layout out of it, and no part of the root is created.
#[test]
fn absolute_destination_outside_the_root_is_refused() {
    let root_scratch = Scratch::new("escape-root");
    let other_scratch = Scratch::new("escape-other");
    let root = root_scratch.path().join("root");
    let destination = other_scratch.path().join("bundle.json");
    let message = refusal(&root, &destination.to_string_lossy());
    assert_eq!(message, OUTSIDE);
    assert!(
        !message.contains(&root.display().to_string()),
        "refusal leaks the export root: {message}"
    );
    assert!(!root.exists(), "{}", root.display());
    assert!(!destination.exists(), "{}", destination.display());
}

#[test]
fn dot_dot_escape_from_the_root_is_refused() {
    let scratch = Scratch::new("dot-dot");
    let root = scratch.path().join("root");
    let destination = format!("{}/../escaped.json", root.display());
    let message = refusal(&root, &destination);
    assert_eq!(message, OUTSIDE);
    assert!(!root.exists(), "{}", root.display());
}

/// The oracle that a canonicalized check used to give away: whether a parent
/// directory exists is answered identically for an existing and a missing one,
/// because the lexical gate refuses both before any filesystem call.
#[test]
fn an_outside_destination_does_not_reveal_whether_its_parent_exists() {
    let scratch = Scratch::new("oracle");
    let root = scratch.path().join("root");
    let present = scratch.path().join("present");
    std::fs::create_dir_all(&present).expect("present parent");
    let existing_parent = present.join("bundle.json");
    let missing_parent = scratch.path().join("absent").join("bundle.json");
    let existing = refusal(&root, &existing_parent.to_string_lossy());
    let missing = refusal(&root, &missing_parent.to_string_lossy());
    assert_eq!(existing, missing, "the two refusals differ");
    assert_eq!(existing, OUTSIDE);
    assert!(
        !existing.contains(&root.display().to_string())
            && !missing.contains(&root.display().to_string()),
        "a refusal leaks the export root: {existing}"
    );
    assert!(!root.exists(), "{}", root.display());
}

#[cfg(unix)]
#[test]
fn symlinked_parent_resolving_outside_the_root_is_refused() {
    let scratch = Scratch::new("symlink");
    let root = scratch.path().join("root");
    let outside = scratch.path().join("outside");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::create_dir_all(&outside).expect("outside");
    std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");
    let destination = root.join("link").join("bundle.json");
    let message = refusal(&root, &destination.to_string_lossy());
    assert_eq!(message, OUTSIDE);
}

#[test]
fn relative_bare_file_name_lands_under_the_root() {
    let scratch = Scratch::new("bare");
    let root = scratch.path().join("root");
    let confined = confine_export_destination(&root, true, "bundle.json")
        .expect("a bare file name is accepted");
    assert!(root.is_dir(), "the root is created on first use");
    assert_eq!(confined, root.join("bundle.json"));
}

/// A destination whose last component is a directory reference names no file,
/// and it is refused before the root is created.
#[test]
fn destination_with_no_file_name_is_refused() {
    let scratch = Scratch::new("no-name");
    let root = scratch.path().join("root");
    for destination in ["..", "/"] {
        let message = refusal(&root, destination);
        assert!(
            message.contains("names no file"),
            "the refusal explains the shape: {message}"
        );
    }
    assert!(!root.exists(), "{}", root.display());
}

/// A missing root can only be created for a bare relative file name: any other
/// shape would be a guess about a directory that does not exist yet.
#[test]
fn a_missing_root_refuses_a_nested_destination_without_creating_anything() {
    let scratch = Scratch::new("nested-missing-root");
    let root = scratch.path().join("root");
    let message = refusal(&root, "nested/bundle.json");
    assert_eq!(message, NO_PARENT);
    assert!(!root.exists(), "{}", root.display());
}

#[test]
fn destination_with_a_missing_parent_directory_is_refused() {
    let scratch = Scratch::new("no-parent");
    let root = scratch.path().join("root");
    std::fs::create_dir_all(&root).expect("root");
    let destination = root.join("absent").join("bundle.json");
    let message = refusal(&root, &destination.to_string_lossy());
    assert_eq!(message, NO_PARENT);
    assert!(!root.join("absent").exists(), "nothing was created");
}

/// A root the deployment points at a directory through a symlink is accepted:
/// where the deployment puts the root is not the caller's problem, and the
/// bundle path stays the one the caller supplied.
#[cfg(unix)]
#[test]
fn a_symlinked_root_that_resolves_to_a_directory_is_accepted() {
    let scratch = Scratch::new("symlink-root");
    let real = scratch.path().join("real-root");
    std::fs::create_dir_all(&real).expect("real root");
    let root = scratch.path().join("root");
    std::os::unix::fs::symlink(&real, &root).expect("symlink root");
    let confined = confine_export_destination(&root, true, "bundle.json")
        .expect("a symlinked root that resolves to a directory is accepted");
    assert_eq!(confined, root.join("bundle.json"));
    assert!(confined.starts_with(&root), "{}", confined.display());
    assert_eq!(confined.parent(), Some(root.as_path()));
}

/// A root that resolves to a non-directory is a deployment fault, so it is an
/// internal error rather than something the caller could fix by changing the
/// destination. The file is left as it was.
#[test]
fn a_root_that_is_a_regular_file_is_refused_as_internal() {
    let scratch = Scratch::new("file-root");
    let root = scratch.path().join("root");
    std::fs::write(&root, b"not a directory").expect("file root");
    let message = internal_refusal(&root, "bundle.json");
    assert!(
        message.contains("is not a directory"),
        "the refusal explains the root shape: {message}"
    );
    assert!(root.is_file(), "{}", root.display());
}

#[cfg(unix)]
#[test]
fn a_root_that_is_a_symlink_to_a_non_directory_is_refused_as_internal() {
    let scratch = Scratch::new("symlink-file-root");
    let real = scratch.path().join("real-file");
    std::fs::write(&real, b"not a directory").expect("real file");
    let root = scratch.path().join("root");
    std::os::unix::fs::symlink(&real, &root).expect("symlink root");
    internal_refusal(&root, "bundle.json");
}

/// The relative fallback tier is not deployment-owned: a stdio server inherits
/// a client project's working directory, so that root is refused as unconfigured
/// and nothing is created.
#[test]
fn an_untrusted_root_is_refused_naming_the_environment_variable() {
    let scratch = Scratch::new("untrusted");
    let root = scratch.path().join("root");
    let err = refused(&root, false, "bundle.json");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(
        err.message.contains("CLIO_EXPORT_ROOT"),
        "the refusal names the setting: {}",
        err.message
    );
    assert!(!root.exists(), "{}", root.display());
}

/// Empty names no file, and whitespace is the same refusal rather than a file
/// literally named a space. Neither reaches the filesystem.
#[test]
fn empty_and_whitespace_only_destinations_are_refused() {
    let scratch = Scratch::new("empty");
    let root = scratch.path().join("root");
    for destination in ["", " ", "\t\n "] {
        let message = refusal(&root, destination);
        assert!(
            message.contains("must not be empty"),
            "the refusal explains the empty destination: {message}"
        );
    }
    assert!(!root.exists(), "{}", root.display());
}

#[cfg(unix)]
#[test]
fn a_created_root_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("mode-new");
    let root = scratch.path().join("fresh-root");
    confine_export_destination(&root, true, "bundle.json").expect("a bare file name is accepted");
    let mode = std::fs::metadata(&root)
        .expect("root metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "{mode:o}");
}

#[cfg(unix)]
#[test]
fn a_pre_existing_root_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("mode-kept");
    let root = scratch.path().join("existing-root");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    confine_export_destination(&root, true, "bundle.json").expect("a bare file name is accepted");
    let mode = std::fs::metadata(&root)
        .expect("root metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "the existing root is never narrowed");
}
