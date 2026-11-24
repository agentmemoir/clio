// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the held output directory: that the pages land in the directory the walk resolved even when the name the caller wrote stops naming it, that a target planted after its check is replaced rather than followed, and that a missing component is created once while a non-directory one is refused.
//!
//! ## Owns
//! - The local scratch directory every test fills
//! - The guards arming the component-swap and planted-target seams, each disarming on drop
//!
//! ## Does not own
//! - The writer under test (the `cli_manual_write_dir` module)
//! - The manual's render, write order, and refusal messages for page content (see `cli_manual_write_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module on what the held directory guarantees, not which pages the manual holds.

use super::*;
use crate::cli_help_markdown::PageMap;
use crate::cli_manual_write::write_manual;
use std::fs;

/// One test's own directory, removed when the guard drops. No other test
/// shares it. This is `cli_manual_write_tests`' `Scratch`: pid plus a counter
/// and nanos keep parallel tests apart, and the drop removes the directory on
/// panic too.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nanos = unix_nanos();
        // Canonicalize the temp root before joining: the writer refuses every
        // symlink path component, and on macOS the system temp directory sits
        // behind symlinks (`/var` and `/tmp` point into `/private`), which
        // would fail the successful writes below for an environmental reason.
        let base = fs::canonicalize(std::env::temp_dir()).expect("the temp root resolves");
        let root = base.join(format!(
            "clio-manual-write-dir-{label}-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("scratch directory can be created");
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The first line of `path`'s bytes.
fn first_line(path: &Path) -> Vec<u8> {
    let bytes = fs::read(path).expect("the file can be read");
    match bytes.iter().position(|byte| *byte == b'\n') {
        Some(end) => bytes[..end].to_vec(),
        None => bytes,
    }
}

/// The sorted names directly inside `dir`.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("the directory can be read")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort_unstable();
    names
}

/// Arm the component-swap seam until the guard drops, so a panicking test cannot leave the seam armed for the next one.
#[cfg(unix)]
struct ArmedComponentSwap;

#[cfg(unix)]
impl ArmedComponentSwap {
    /// Arm the seam: the next promote moves the held directory to `aside` and puts a symlink to `decoy` where it was.
    fn arm(aside: &Path, decoy: &Path) -> Self {
        *SWAP_BEFORE_PROMOTE
            .lock()
            .expect("the swap seam lock is not poisoned") =
            Some((aside.to_path_buf(), decoy.to_path_buf()));
        Self
    }
}

#[cfg(unix)]
impl Drop for ArmedComponentSwap {
    fn drop(&mut self) {
        *SWAP_BEFORE_PROMOTE
            .lock()
            .expect("the swap seam lock is not poisoned") = None;
    }
}

/// Arm the planted-target seam until the guard drops, so a panicking test cannot leave the seam armed for the next one.
#[cfg(unix)]
struct ArmedPlantedTarget;

#[cfg(unix)]
impl ArmedPlantedTarget {
    /// Arm the seam: the next temp write puts a symlink to `outside` at `page` in the held directory.
    fn arm(outside: &Path, page: &str) -> Self {
        *PLANT_AFTER_CHECK
            .lock()
            .expect("the plant seam lock is not poisoned") =
            Some((outside.to_path_buf(), OsString::from(page)));
        Self
    }
}

#[cfg(unix)]
impl Drop for ArmedPlantedTarget {
    fn drop(&mut self) {
        *PLANT_AFTER_CHECK
            .lock()
            .expect("the plant seam lock is not poisoned") = None;
    }
}

#[cfg(unix)]
#[test]
fn the_held_directory_keeps_naming_the_checked_directory() {
    let scratch = Scratch::new("held");
    let root = scratch.root();
    let real = root.join("real");
    let renamed = root.join("renamed");
    let decoy = root.join("decoy");
    fs::create_dir(&real).expect("the checked directory");
    fs::create_dir(&decoy).expect("the directory that takes its name");

    let dir = OutDir::open(&real).expect("the checked directory opens");
    // The name the caller wrote no longer resolves to the checked directory.
    fs::rename(&real, &renamed).expect("the checked directory moves");
    std::os::unix::fs::symlink(&decoy, &real).expect("the decoy link takes its name");

    dir.check_target("index.md")
        .expect("the target is absent in the held directory");
    let tmp = dir
        .write_temp("index.md", b"generated page\n")
        .expect("the temp writes in the held directory");
    dir.promote(&tmp, "index.md")
        .expect("the promote lands in the held directory");

    assert_eq!(
        fs::read(renamed.join("index.md")).expect("the page in the checked directory"),
        b"generated page\n",
        "the page landed in the directory the walk resolved"
    );
    assert_eq!(
        names_in(&decoy),
        Vec::<String>::new(),
        "the symlink that took the name was never written through"
    );
}

#[cfg(unix)]
#[test]
fn a_component_swapped_before_the_write_lands_in_the_checked_directory() {
    let scratch = Scratch::new("swapped");
    let root = scratch.root();
    let checked = root.join("checked");
    let aside = root.join("aside");
    let decoy = root.join("decoy");
    fs::create_dir(&checked).expect("the checked directory");
    fs::create_dir(&decoy).expect("the directory that takes its name");

    let dir = OutDir::open(&checked).expect("the checked directory opens");
    dir.check_target("index.md")
        .expect("the target is absent in the held directory");
    // Swapped after the walk and after the check, so only the held inode still names the checked directory.
    let _armed = ArmedComponentSwap::arm(&aside, &decoy);

    let tmp = dir
        .write_temp("index.md", b"generated page\n")
        .expect("the temp writes in the held directory");
    dir.promote(&tmp, "index.md")
        .expect("the promote lands in the held directory");

    assert_eq!(
        fs::read(aside.join("index.md")).expect("the page in the checked directory"),
        b"generated page\n",
        "the page landed in the checked directory, not at the name it was reached by"
    );
    assert_eq!(
        names_in(&decoy),
        Vec::<String>::new(),
        "the decoy directory that took the name was never written through"
    );
}

#[cfg(unix)]
#[test]
fn a_page_target_planted_as_a_symlink_after_the_check_is_replaced_not_followed() {
    let scratch = Scratch::new("planted");
    let root = scratch.root();
    let out = root.join("out");
    let outside = root.join("outside.md");
    fs::create_dir(&out).expect("the output directory");

    let dir = OutDir::open(&out).expect("the output directory opens");
    dir.check_target("index.md")
        .expect("the target is absent at the check");
    // The page name becomes a symlink out of `--out` between the check and the write.
    let _armed = ArmedPlantedTarget::arm(&outside, "index.md");

    let page = format!("{GENERATED_MARKER}\ngenerated page\n");
    let tmp = dir
        .write_temp("index.md", page.as_bytes())
        .expect("the temp is a fresh name, not the planted one");
    dir.promote(&tmp, "index.md")
        .expect("the promote replaces the planted link");

    let target = out.join("index.md");
    assert_eq!(
        fs::read(&target).expect("the page target"),
        page.as_bytes(),
        "the page landed at the page target"
    );
    assert!(
        !fs::symlink_metadata(&target)
            .expect("the page target")
            .file_type()
            .is_symlink(),
        "the planted link was replaced, so the target is a real file"
    );
    assert!(
        !outside.exists(),
        "the file outside `--out` was not created by the page"
    );
}

#[test]
fn an_out_path_through_a_missing_component_creates_it_once() {
    let scratch = Scratch::new("missing-component");
    let root = scratch.root();
    let out = root.join("nested").join("deep").join("manual");
    write_manual(&out).expect("a missing component is created and the manual writes");

    assert!(out.is_dir(), "the missing components were created");
    assert_eq!(
        names_in(&root.join("nested").join("deep")),
        vec!["manual".to_owned()],
        "the walk created one directory in the parent, not a leftover beside it"
    );
    let index = out.join(PageMap::index_page());
    assert_eq!(
        first_line(&index),
        GENERATED_MARKER.as_bytes(),
        "the pages landed inside the created component"
    );
}

#[test]
fn a_component_that_is_not_a_directory_is_refused() {
    let scratch = Scratch::new("not-a-directory");
    let root = scratch.root();
    let blocker = root.join("blocker");
    fs::write(&blocker, "not a directory").expect("the blocking file");
    let out = blocker.join("manual");

    let error = write_manual(&out).expect_err("a path through a file is refused");

    assert_eq!(error.code(), "usage", "the refusal is a usage error");
    assert!(
        error.message().contains("is not a directory"),
        "the refusal names the component: {}",
        error.message()
    );
    assert!(!out.exists(), "the refused output path was not created");
}
