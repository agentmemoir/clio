// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the staging-directory mechanics: owner-only creation,
//! the fail-closed refusal of an unsafe existing directory, the
//! unpredictable per-run staging-directory naming, the staged-file removal
//! that drops an emptied staging directory, and the leftover sweep across
//! per-run and legacy staging directories.
//!
//! ## Owns
//! - The scratch-base fixture and the assertions over staging creation,
//!   refusal, removal, and sweep behaviour
//!
//! ## Does not own
//! - The delivery-sequence assertions (`export_custody_tests` owns those)
//! - The reconcile assertions (`export_custody_reconcile_tests` owns those)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{
    STAGING_DIR_NAMES, remove_staged, stage_staging, sweep_staging_leftovers, unique_staging_dir,
};
use crate::test_unique::unique;
use std::fs;
use std::path::PathBuf;

/// A scratch base directory under the system temp root, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(unique(name));
        fs::create_dir_all(&path).expect("scratch base created");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn fresh_staging_directory_is_created_owner_only() {
    let scratch = Scratch::new("stage-owner-only");
    let dir = scratch.0.join("staging");
    let staged = stage_staging(&dir, "export-1-0", b"bytes").expect("staging succeeds");
    assert_eq!(fs::read(&staged).expect("staged read"), b"bytes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dir)
            .expect("staging directory exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "staging directory mode");
    }
}

#[test]
fn existing_directory_with_wide_permissions_is_refused() {
    let scratch = Scratch::new("stage-refuse-wide");
    let dir = scratch.0.join("staging");
    fs::create_dir(&dir).expect("wide directory created");
    let err = stage_staging(&dir, "export-1-0", b"bytes").expect_err("staging refused");
    assert!(
        err.to_string().contains("refusing staging directory"),
        "refusal names the unsafe directory: {err}"
    );
}

#[test]
fn existing_owner_only_directory_is_reused() {
    let scratch = Scratch::new("stage-reuse");
    let dir = scratch.0.join("staging");
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&dir).expect("owner-only directory created");
    }
    #[cfg(not(unix))]
    fs::create_dir(&dir).expect("staging directory created");
    stage_staging(&dir, "export-1-0", b"first").expect("first staging succeeds");
    stage_staging(&dir, "export-1-1", b"second").expect("second staging reuses the directory");
}

#[cfg(unix)]
#[test]
fn link_at_the_staging_directory_path_is_refused() {
    let scratch = Scratch::new("stage-refuse-link");
    let target = scratch.0.join("target");
    fs::create_dir(&target).expect("target directory created");
    let dir = scratch.0.join("staging");
    std::os::unix::fs::symlink(&target, &dir).expect("link planted");
    let err = stage_staging(&dir, "export-1-0", b"bytes").expect_err("staging refused");
    assert!(
        err.to_string().contains("refusing staging directory"),
        "a link never becomes a staging directory: {err}"
    );
}

#[test]
fn unique_staging_dir_names_differ_and_keep_the_product_prefix() {
    let product = STAGING_DIR_NAMES[0];
    let first = unique_staging_dir(product);
    let second = unique_staging_dir(product);
    assert_ne!(first, second, "two calls never share a name");
    let name = first
        .file_name()
        .and_then(|name| name.to_str())
        .expect("staging name is valid unicode")
        .to_owned();
    let suffix = &name[product.len() + 1..];
    assert!(name.starts_with(product) && suffix.len() == 32, "{name}");
}

#[test]
fn remove_staged_drops_the_staging_directory_when_empty() {
    let scratch = Scratch::new("stage-remove-empty");
    let dir = scratch.0.join("staging");
    let staged = stage_staging(&dir, "export-2-0", b"bytes").expect("staging succeeds");
    remove_staged(&staged);
    assert!(!staged.exists(), "the staged file is removed");
    assert!(!dir.exists(), "the emptied staging directory is dropped");
}

#[test]
fn remove_staged_keeps_a_staging_directory_that_still_holds_files() {
    let scratch = Scratch::new("stage-remove-occupied");
    let dir = scratch.0.join("staging");
    let staged = stage_staging(&dir, "export-2-0", b"bytes").expect("staging succeeds");
    let other = stage_staging(&dir, "export-2-1", b"bytes").expect("staging succeeds");
    remove_staged(&staged);
    assert!(!staged.exists(), "the staged file is removed");
    assert!(
        other.exists() && dir.exists(),
        "a directory with files left stays"
    );
}

#[test]
fn sweep_removes_the_exact_staged_name_in_a_per_run_dir_and_leaves_the_rest() {
    let scratch = Scratch::new("sweep-per-run");
    let dir = scratch.0.join(format!("{}-1a2b", STAGING_DIR_NAMES[1]));
    fs::create_dir(&dir).expect("per-run staging directory created");
    let staged = dir.join("review-export-3-0.staging");
    fs::write(&staged, b"leftover").expect("leftover seeded");
    let foreign = dir.join("review-export-3-1.staging");
    fs::write(&foreign, b"foreign").expect("foreign file seeded");
    sweep_staging_leftovers(&scratch.0, "review-export-3-0");
    assert!(!staged.exists(), "the managed leftover is removed");
    assert!(
        foreign.exists(),
        "only exact managed names are swept: another action's staged file stays"
    );
}

#[test]
fn sweep_drops_a_per_run_dir_its_leftover_removal_emptied() {
    let scratch = Scratch::new("sweep-empty-dir");
    let dir = scratch.0.join(format!("{}-3c4d", STAGING_DIR_NAMES[0]));
    fs::create_dir(&dir).expect("per-run staging directory created");
    let staged = dir.join("export-3-2.staging");
    fs::write(&staged, b"leftover").expect("leftover seeded");
    sweep_staging_leftovers(&scratch.0, "export-3-2");
    assert!(!dir.exists(), "the emptied staging directory is dropped");
}

#[test]
fn sweep_covers_the_legacy_fixed_name_directory() {
    let scratch = Scratch::new("sweep-legacy");
    let dir = scratch.0.join(STAGING_DIR_NAMES[0]);
    fs::create_dir(&dir).expect("legacy staging directory created");
    let staged = dir.join("export-4-0.staging");
    fs::write(&staged, b"leftover").expect("leftover seeded");
    sweep_staging_leftovers(&scratch.0, "export-4-0");
    assert!(!staged.exists(), "the legacy leftover is removed");
}

#[cfg(unix)]
#[test]
fn sweep_never_follows_a_link_at_the_staging_directory_name() {
    let scratch = Scratch::new("sweep-link");
    let outside = scratch.0.join("outside");
    fs::create_dir(&outside).expect("outside directory created");
    let staged = outside.join("export-4-1.staging");
    fs::write(&staged, b"leftover").expect("leftover seeded");
    std::os::unix::fs::symlink(
        &outside,
        scratch.0.join(format!("{}-linked", STAGING_DIR_NAMES[0])),
    )
    .expect("link planted");
    sweep_staging_leftovers(&scratch.0, "export-4-1");
    assert!(staged.exists(), "a leftover behind a link stays");
}
