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

use super::confine_export_destination;
use clio_types::ErrorCode;
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

/// The root every assertion compares against, canonicalized because the OS temp
/// directory is reached through a symlink on some platforms.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().expect("canonical root")
}

/// Assert one refusal and return its message for further inspection.
fn refusal(root: &Path, destination: &str) -> String {
    let err = confine_export_destination(root, destination)
        .expect_err("destination outside the export root must be refused");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{}", err.message);
    err.message
}

#[test]
fn destination_inside_the_root_is_accepted_and_normalised() {
    let scratch = Scratch::new("inside");
    let root = scratch.path().join("root");
    std::fs::create_dir_all(root.join("nested")).expect("root");
    let destination = root.join("./nested/bundle.json");
    let confined = confine_export_destination(&root, &destination.to_string_lossy())
        .expect("a destination inside the root is accepted");
    assert!(confined.is_absolute(), "{}", confined.display());
    assert_eq!(
        confined,
        canonical(&root).join("nested").join("bundle.json")
    );
}

#[test]
fn absolute_destination_outside_the_root_is_refused() {
    let root_scratch = Scratch::new("escape-root");
    let other_scratch = Scratch::new("escape-other");
    let root = root_scratch.path().join("root");
    std::fs::create_dir_all(&root).expect("root");
    let destination = other_scratch.path().join("bundle.json");
    let message = refusal(&root, &destination.to_string_lossy());
    assert!(
        message.contains(&root.display().to_string()),
        "refusal names the export root: {message}"
    );
    assert!(!destination.exists(), "{}", destination.display());
}

#[test]
fn dot_dot_escape_from_the_root_is_refused() {
    let scratch = Scratch::new("dot-dot");
    let root = scratch.path().join("root");
    std::fs::create_dir_all(&root).expect("root");
    let destination = format!("{}/../escaped.json", root.display());
    let message = refusal(&root, &destination);
    assert!(
        message.contains(&root.display().to_string()),
        "refusal names the export root: {message}"
    );
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
    refusal(&root, &destination.to_string_lossy());
}

#[test]
fn relative_bare_file_name_lands_under_the_root() {
    let scratch = Scratch::new("bare");
    let root = scratch.path().join("root");
    let confined =
        confine_export_destination(&root, "bundle.json").expect("a bare file name is accepted");
    assert!(root.is_dir(), "the root is created on first use");
    assert_eq!(confined, canonical(&root).join("bundle.json"));
}

#[test]
fn destination_with_no_file_name_is_refused() {
    let scratch = Scratch::new("no-name");
    let root = scratch.path().join("root");
    std::fs::create_dir_all(&root).expect("root");
    let root_text = root.to_string_lossy().into_owned();
    for destination in [".", "..", root_text.as_str()] {
        refusal(&root, destination);
    }
}

#[test]
fn destination_with_a_missing_parent_directory_is_refused() {
    let scratch = Scratch::new("no-parent");
    let root = scratch.path().join("root");
    std::fs::create_dir_all(&root).expect("root");
    let destination = root.join("absent").join("bundle.json");
    let message = refusal(&root, &destination.to_string_lossy());
    assert!(
        message.contains("does not exist"),
        "refusal explains the missing parent: {message}"
    );
    assert!(!root.join("absent").exists(), "nothing was created");
}

#[cfg(unix)]
#[test]
fn a_created_root_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("mode-new");
    let root = scratch.path().join("fresh-root");
    confine_export_destination(&root, "bundle.json").expect("a bare file name is accepted");
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
    confine_export_destination(&root, "bundle.json").expect("a bare file name is accepted");
    let mode = std::fs::metadata(&root)
        .expect("root metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "the existing root is never narrowed");
}
