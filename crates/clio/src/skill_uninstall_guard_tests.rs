// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the path rules the skill remover trusts: which marker keys are plain relative paths, which package directories a harness or the portable root resolves to, whether a recorded path or an ancestor on the way to it is a symbolic link, and the plain failure for a path that cannot be inspected.
//!
//! ## Owns
//! - Every assertion over `marker_key_is_relative`, `package_dirs`, `symlink_component`, and `ancestor_symlink`, built from a scratch tree the test creates and removes
//!
//! ## Does not own
//! - The path rules under test (`skill_uninstall_guard`)
//! - What a removal decides or prints (`skill_uninstall`, `skill_uninstall_text`)
//!
//! ## Boundary
//!
//! Test-only. Every path lives under one unique directory the test owns, so no test reads the real home directory or depends on another test's state.
//!
//! Keep this module focused on the path rules, with one scratch tree per test.

use super::{ancestor_symlink, marker_key_is_relative, package_dirs, symlink_component};
use crate::agent_harness::{HarnessRecord, Mechanism, ReadBack};
use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use crate::skill_install::SKILL_DIR_NAME;
use crate::skill_marker::SKILL_MARKER_FILE;
use crate::skill_uninstall::UninstallError;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Nanoseconds plus a process-global counter: two scratch names never collide.
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// The workspace test temp root, so a scratch tree never lands in the system temp directory.
const TEST_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/tmp");

/// One test's own directory under the workspace test temp root, removed when the guard drops.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = Path::new(TEST_ROOT).join(format!(
            "clio-skill-uninstall-guard-{label}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("the scratch directory can be created");
        Self { root }
    }

    /// One directory inside the scratch root, created.
    fn dir(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(&path).expect("the scratch subdirectory can be created");
        path
    }

    /// One file inside the scratch root, with its parent created and one byte written.
    fn file(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("the file's parent can be created");
        }
        fs::write(&path, b"x").expect("the scratch file can be written");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A Linux-layout environment rooted at `home`.
fn env(home: &Path) -> DirectWriteEnv {
    DirectWriteEnv::with(OsLayout::Linux, home.to_path_buf(), None, None, None)
}

/// A record that stands for a harness with a native root and the portable root.
fn claude_record() -> HarnessRecord {
    HarnessRecord {
        name: "claude",
        aliases: &[],
        mechanism: Mechanism::Delegate,
        config_file: ".claude.json",
        read_back: ReadBack::Json {
            key_prefixes: &[&["mcpServers"]],
        },
    }
}

/// A record that stands for a harness reading the portable root alone.
fn cursor_record() -> HarnessRecord {
    HarnessRecord {
        name: "cursor",
        aliases: &[],
        mechanism: Mechanism::DirectWrite,
        config_file: ".cursor/mcp.json",
        read_back: ReadBack::Json {
            key_prefixes: &[&["mcpServers"]],
        },
    }
}

#[test]
fn a_plain_and_a_nested_key_are_relative() {
    assert!(marker_key_is_relative("SKILL.md"));
    assert!(marker_key_is_relative("references/one/two.md"));
}

#[test]
fn an_empty_key_is_not_relative() {
    assert!(!marker_key_is_relative(""));
}

#[test]
fn the_marker_name_is_not_relative() {
    assert!(!marker_key_is_relative(SKILL_MARKER_FILE));
}

#[test]
fn an_absolute_key_is_not_relative() {
    assert!(!marker_key_is_relative("/etc/passwd"));
}

#[test]
fn a_parent_or_current_step_is_not_relative() {
    assert!(!marker_key_is_relative(".."));
    assert!(!marker_key_is_relative("."));
    assert!(!marker_key_is_relative("../escape.md"));
    assert!(!marker_key_is_relative("references/../../escape.md"));
}

#[test]
fn a_rooted_double_slash_key_is_not_relative() {
    // A leading `//` makes the first component a root, not a name.
    assert!(!marker_key_is_relative("//server/share.md"));
}

#[cfg(windows)]
#[test]
fn a_windows_prefix_is_not_relative() {
    assert!(!marker_key_is_relative("C:\\skills\\SKILL.md"));
}

#[test]
fn no_record_resolves_the_portable_package() {
    let scratch = Scratch::new("portable");
    let packages = package_dirs(None, &env(&scratch.root)).expect("the portable root resolves");
    assert_eq!(
        packages,
        vec![scratch.root.join(".agents/skills").join(SKILL_DIR_NAME)]
    );
}

#[test]
fn a_harness_resolves_its_native_package_before_the_portable_one() {
    let scratch = Scratch::new("native");
    let packages = package_dirs(Some(&claude_record()), &env(&scratch.root))
        .expect("the claude roots resolve");
    assert_eq!(
        packages,
        vec![
            scratch.root.join(".claude/skills").join(SKILL_DIR_NAME),
            scratch.root.join(".agents/skills").join(SKILL_DIR_NAME),
        ]
    );
}

#[test]
fn a_portable_only_harness_resolves_one_package() {
    let scratch = Scratch::new("portable-only");
    let packages = package_dirs(Some(&cursor_record()), &env(&scratch.root))
        .expect("the cursor root resolves");
    assert_eq!(
        packages,
        vec![scratch.root.join(".agents/skills").join(SKILL_DIR_NAME)]
    );
}

#[test]
fn an_unknown_home_for_a_harness_is_an_io_failure_that_names_it() {
    let error = package_dirs(Some(&claude_record()), &env(Path::new("")))
        .expect_err("an unknown home cannot resolve a root");
    let UninstallError::Io { detail } = error else {
        panic!("expected an IO failure for an unknown home");
    };
    assert!(detail.contains("claude"), "{detail}");
    assert!(detail.contains("home directory is unknown"), "{detail}");
}

#[test]
fn an_unknown_home_for_the_portable_root_is_an_io_failure() {
    let error = package_dirs(None, &env(Path::new("")))
        .expect_err("an unknown home cannot resolve the portable root");
    let UninstallError::Io { detail } = error else {
        panic!("expected an IO failure for an unknown home");
    };
    assert!(detail.contains("portable"), "{detail}");
    assert!(detail.contains("home directory is unknown"), "{detail}");
}

#[test]
fn a_real_tree_has_no_symlink_component() {
    let scratch = Scratch::new("clean-tree");
    let dir = scratch.dir("package");
    scratch.file("package/references/one.md");
    assert!(!symlink_component(&dir, "references/one.md").expect("the path is inspectable"));
}

#[test]
fn a_missing_component_is_not_a_symlink() {
    let scratch = Scratch::new("missing-component");
    let dir = scratch.dir("package");
    assert!(
        !symlink_component(&dir, "references/absent.md").expect("a missing path is not a link")
    );
}

#[test]
fn a_missing_top_component_is_not_a_symlink() {
    let scratch = Scratch::new("missing-top");
    let dir = scratch.dir("package");
    assert!(!symlink_component(&dir, "absent").expect("a missing path is not a link"));
}

#[cfg(unix)]
#[test]
fn a_linked_recorded_asset_is_a_symlink_component() {
    let scratch = Scratch::new("linked-asset");
    let dir = scratch.dir("package");
    scratch.file("outside.md");
    std::os::unix::fs::symlink(scratch.root.join("outside.md"), dir.join("SKILL.md"))
        .expect("the asset becomes a link");
    assert!(symlink_component(&dir, "SKILL.md").expect("the link is inspectable"));
}

#[cfg(unix)]
#[test]
fn a_linked_intermediate_directory_is_a_symlink_component() {
    let scratch = Scratch::new("linked-dir");
    let dir = scratch.dir("package");
    scratch.file("real/one.md");
    std::os::unix::fs::symlink(scratch.root.join("real"), dir.join("references"))
        .expect("the directory becomes a link");
    assert!(symlink_component(&dir, "references/one.md").expect("the link is inspectable"));
}

#[cfg(unix)]
#[test]
fn a_regular_file_in_a_directory_position_is_an_io_failure() {
    let scratch = Scratch::new("file-in-position");
    let dir = scratch.dir("package");
    scratch.file("package/references");
    let error = symlink_component(&dir, "references/one.md")
        .expect_err("a file cannot be walked through as a directory");
    let UninstallError::Io { detail } = error else {
        panic!("expected an IO failure for a path through a file");
    };
    assert!(detail.contains("failed to inspect"), "{detail}");
    assert!(detail.contains("references"), "{detail}");
}

#[test]
fn a_tree_without_links_has_no_ancestor_symlink() {
    let scratch = Scratch::new("clean-ancestors");
    let package = scratch.dir(".agents/skills/clio-mcp");
    assert_eq!(
        ancestor_symlink(&scratch.root, &package).expect("the tree is inspectable"),
        None
    );
}

#[test]
fn a_linked_package_directory_is_reported_as_an_ancestor_symlink() {
    let scratch = Scratch::new("linked-package");
    let package = scratch.root.join(".agents/skills/clio-mcp");
    let real = scratch.dir("real-package");
    fs::create_dir_all(package.parent().expect("the skill root exists"))
        .expect("the skill root can be created");
    std::os::unix::fs::symlink(&real, &package).expect("the package becomes a link");
    assert_eq!(
        ancestor_symlink(&scratch.root, &package).expect("the link is inspectable"),
        Some(package)
    );
}

#[test]
fn a_linked_intermediate_component_is_reported_with_its_path() {
    let scratch = Scratch::new("linked-skill-root");
    let claude_dir = scratch.root.join(".claude");
    let real = scratch.dir("real-claude");
    std::os::unix::fs::symlink(&real, &claude_dir).expect("the skill root becomes a link");
    let package = claude_dir.join("skills").join(SKILL_DIR_NAME);
    fs::create_dir_all(real.join("skills").join(SKILL_DIR_NAME))
        .expect("the package exists behind the link");
    assert_eq!(
        ancestor_symlink(&scratch.root, &package).expect("the link is inspectable"),
        Some(claude_dir)
    );
}

#[test]
fn a_missing_component_stops_the_ancestor_walk() {
    let scratch = Scratch::new("missing-ancestor");
    let package = scratch.root.join(".agents/skills/clio-mcp");
    assert_eq!(
        ancestor_symlink(&scratch.root, &package).expect("a missing path is not a failure"),
        None
    );
}

#[cfg(unix)]
#[test]
fn a_regular_file_on_the_ancestor_walk_is_an_io_failure() {
    let scratch = Scratch::new("file-ancestor");
    let blocker = scratch.file("blocked");
    let error = ancestor_symlink(&scratch.root, &blocker.join("skills"))
        .expect_err("a file cannot be walked through as a directory");
    let UninstallError::Io { detail } = error else {
        panic!("expected an IO failure for a path through a file");
    };
    assert!(detail.contains("failed to inspect"), "{detail}");
    assert!(detail.contains("skills"), "{detail}");
}

#[cfg(unix)]
#[test]
fn a_linked_home_is_not_a_refusal() {
    let scratch = Scratch::new("linked-home");
    let real_home = scratch.dir("real-home");
    let linked_home = scratch.root.join("linked-home");
    std::os::unix::fs::symlink(&real_home, &linked_home).expect("the home becomes a link");
    let package = real_home.join(".agents/skills/clio-mcp");
    fs::create_dir_all(&package).expect("the package can be created");
    assert_eq!(
        ancestor_symlink(&linked_home, &linked_home.join(".agents/skills/clio-mcp"))
            .expect("the tree is inspectable"),
        None
    );
}
