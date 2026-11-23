// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the descriptor-relative delivery: that an ancestor swapped
//! for a link between the confinement check and the link cannot place the
//! bundle outside the export root, that a nested destination under a trusted
//! root still lands inside the real directory it names, and that every
//! fail-closed branch of the parent walk and the final-name classification
//! refuses with its own error.
//!
//! ## Owns
//! - The swap regression: the refused delivery and the untouched outside
//!   directory
//! - The positive control: the same tree without the swap still delivers
//! - The parent walk's fail-closed branches: no components, a skipped `.`, a
//!   parent outside the root, a `..`, a root component, and no parent at all
//! - The held-descriptor classification: a directory and a symlink at the
//!   destination, a symlinked parent, a non-directory child, a missing root,
//!   and a name too long to inspect
//!
//! ## Does not own
//! - The delivery sequence that reaches this module (`export_custody` owns it)
//! - The scratch fixture (`export_custody_tests` owns it, shared here)
//!
//! ## Boundary
//!
//! Tests here break when the mechanism that reaches the destination directory
//! changes. They assert the security property, not the internal steps.
//!
//! Test-only. Never compiled into production builds.

#![cfg(unix)]

use super::{OUTSIDE, deliver_relative, relative_parent};
use crate::export_custody::attempt_delivery;
use crate::export_custody::export_custody_tests::Scratch;
use crate::export_custody::{DeliveryFailure, DeliveryRefusal};
use clio_types::{AmError, ErrorCode};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A short name for each failure shape, so a mismatch names what it saw
/// without needing `Debug` on the failure enum.
fn kind(failure: &DeliveryFailure) -> &'static str {
    match failure {
        DeliveryFailure::Refused(_) => "refused",
        DeliveryFailure::BeforeWrite(_) => "before-write",
        DeliveryFailure::AfterWrite(_) => "after-write",
    }
}

/// The error a before-write failure carries.
fn before_write(failure: DeliveryFailure) -> AmError {
    match failure {
        DeliveryFailure::BeforeWrite(error) => error,
        ref other => panic!("expected a before-write failure, got {}", kind(other)),
    }
}

/// The refusal a refused failure carries.
fn refused(failure: &DeliveryFailure) -> DeliveryRefusal {
    match failure {
        DeliveryFailure::Refused(refusal) => *refusal,
        other => panic!("expected a refusal, got {}", kind(other)),
    }
}

/// The components a walk that must succeed produced.
fn walked(root: &Path, destination: &Path) -> Vec<OsString> {
    match relative_parent(root, destination) {
        Ok(names) => names,
        Err(failure) => panic!("the walk was expected to succeed, got {}", kind(&failure)),
    }
}

/// The failure a walk that must refuse produced.
fn walk_refusal(root: &Path, destination: &Path) -> DeliveryFailure {
    match relative_parent(root, destination) {
        Ok(names) => panic!("the walk was expected to refuse, got {names:?}"),
        Err(failure) => failure,
    }
}

/// Every entry in `dir`, sorted.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read the directory")
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    found.sort();
    found
}

/// A scratch root holding its real `nested` directory and its `outside`
/// sibling, which no destination under the root may reach.
fn root_and_outside(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let root = scratch.root.join("root");
    std::fs::create_dir_all(root.join("nested")).expect("create nested");
    let outside = scratch.root.join("outside");
    std::fs::create_dir_all(&outside).expect("create outside");
    (root, outside)
}

/// A destination directly in the root strips to no components: the link lands
/// in the root's own descriptor.
#[test]
fn a_parent_that_is_the_root_strips_to_no_components() {
    let scratch = Scratch::new();
    let (root, _outside) = root_and_outside(&scratch);
    let names = walked(&root, &root.join("out.bin"));
    assert!(names.is_empty(), "the root itself needs no component walk");
}

/// A `.` component in the stripped parent is skipped, not pushed as a name.
#[test]
fn a_cur_dir_component_in_the_stripped_parent_is_skipped() {
    let names = walked(Path::new(""), Path::new("./nested/out.bin"));
    assert_eq!(
        names,
        vec![OsString::from("nested")],
        "only the real component survives the walk"
    );
}

/// A destination whose parent is not under the root refuses as outside it.
#[test]
fn a_parent_outside_the_root_refuses_as_outside_it() {
    let scratch = Scratch::new();
    let (root, outside) = root_and_outside(&scratch);
    let error = before_write(walk_refusal(&root, &outside.join("out.bin")));
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, OUTSIDE, "the refusal names the root escape");
}

/// A `..` component in the stripped parent refuses as outside the root.
#[test]
fn a_parent_dir_component_refuses_as_outside_the_root() {
    let scratch = Scratch::new();
    let (root, _outside) = root_and_outside(&scratch);
    let destination = root.join("nested").join("..").join("out.bin");
    let error = before_write(walk_refusal(&root, &destination));
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, OUTSIDE);
}

/// An absolute parent, which carries a root component, refuses as outside the
/// root rather than being re-anchored.
#[test]
fn a_root_dir_component_refuses_as_outside_the_root() {
    let error = before_write(walk_refusal(Path::new(""), Path::new("/absolute/out.bin")));
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, OUTSIDE);
}

/// A destination with no parent at all refuses as an invalid argument.
#[test]
fn a_destination_with_no_parent_is_refused() {
    let scratch = Scratch::new();
    let (root, _outside) = root_and_outside(&scratch);
    let error = before_write(walk_refusal(&root, Path::new("out.bin")));
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, "destination has no parent");
}

/// A destination that names no final component refuses before the write and
/// leaves the directory it resolves into empty.
#[test]
fn a_destination_naming_no_file_is_refused_before_the_write() {
    let scratch = Scratch::new();
    let (root, _outside) = root_and_outside(&scratch);
    let destination = root.join("nested").join("..");
    let error = before_write(
        deliver_relative(&root, &destination, "act-no-name", b"payload")
            .expect_err("a directory path names no file"),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, "export destination names no file");
    assert!(
        entries(&root.join("nested")).is_empty(),
        "nothing was created in the directory the walk reached"
    );
}

/// A directory at the destination is refused as a non-regular file and stays a
/// directory.
#[test]
fn a_directory_at_the_destination_is_refused_through_the_descriptor() {
    let scratch = Scratch::new();
    let (root, _outside) = root_and_outside(&scratch);
    let destination = root.join("dir-dest");
    std::fs::create_dir(&destination).expect("create the directory destination");
    let failure = deliver_relative(&root, &destination, "act-dir", b"payload")
        .expect_err("a directory is not a regular file");
    let refusal = refused(&failure);
    assert_eq!(refusal, DeliveryRefusal::DestinationNotRegularFile);
    assert!(destination.is_dir(), "the directory is left in place");
}

/// A symlink at the destination is classified without following it: it refuses
/// as a non-regular file and its target keeps its bytes.
#[test]
fn a_symlink_at_the_destination_is_refused_without_being_followed() {
    let scratch = Scratch::new();
    let (root, outside) = root_and_outside(&scratch);
    let target = outside.join("target.bin");
    std::fs::write(&target, b"target-bytes").expect("write the symlink target");
    let destination = root.join("out.bin");
    std::os::unix::fs::symlink(&target, &destination).expect("create the destination symlink");
    let failure = deliver_relative(&root, &destination, "act-link", b"payload")
        .expect_err("a symlink is not a regular file");
    let refusal = refused(&failure);
    assert_eq!(refusal, DeliveryRefusal::DestinationNotRegularFile);
    assert!(
        std::fs::symlink_metadata(&destination)
            .expect("inspect the destination")
            .file_type()
            .is_symlink(),
        "the symlink itself is still a symlink"
    );
    assert_eq!(
        std::fs::read(&target).expect("read the target"),
        b"target-bytes",
        "the link was classified, never followed and never written through"
    );
}

/// A parent component that is a symlink to a directory is refused by the
/// `NOFOLLOW` walk, and nothing lands in the directory it points at.
#[test]
fn a_symlinked_parent_is_refused_and_never_followed() {
    let scratch = Scratch::new();
    let (root, outside) = root_and_outside(&scratch);
    std::fs::remove_dir(root.join("nested")).expect("remove the real nested");
    std::os::unix::fs::symlink(&outside, root.join("nested")).expect("swap in the link");
    let error = before_write(
        deliver_relative(
            &root,
            &root.join("nested").join("out.bin"),
            "act-swap",
            b"payload",
        )
        .expect_err("the walk never follows a link"),
    );
    assert!(
        error.message.contains("open the export destination parent"),
        "the walk itself refused: {}",
        error.message
    );
    assert!(
        entries(&outside).is_empty(),
        "the directory the link points at stays empty"
    );
}

/// A parent component that is a regular file cannot be opened as a directory,
/// so the walk refuses before the write.
#[test]
fn a_child_that_is_not_a_directory_is_refused_at_the_walk() {
    let scratch = Scratch::new();
    let (root, _outside) = root_and_outside(&scratch);
    let child = root.join("child");
    std::fs::write(&child, b"not-a-dir").expect("write the blocking child");
    let error = before_write(
        deliver_relative(&root, &child.join("out.bin"), "act-child", b"payload")
            .expect_err("a regular file is not a directory"),
    );
    assert!(
        error.message.contains("open the export destination parent"),
        "the walk refused: {}",
        error.message
    );
    assert_eq!(
        std::fs::read(&child).expect("read the child"),
        b"not-a-dir",
        "the blocking file is untouched"
    );
}

/// A root that does not exist is refused when it is opened, before any write.
#[test]
fn a_missing_root_is_refused_before_the_write() {
    let scratch = Scratch::new();
    let root = scratch.root.join("absent-root");
    let error = before_write(
        deliver_relative(
            &root,
            &root.join("nested").join("out.bin"),
            "act-gone",
            b"payload",
        )
        .expect_err("the root cannot be opened"),
    );
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(
        error.message.contains("open the export root"),
        "the refusal names the root open: {}",
        error.message
    );
    assert!(!root.exists(), "the refused delivery creates no root");
}

/// A root that is a regular file is refused when it is opened as a directory,
/// before any write.
#[test]
fn a_root_that_is_not_a_directory_is_refused_before_the_write() {
    let scratch = Scratch::new();
    let root = scratch.root.join("root-file");
    std::fs::write(&root, b"not-a-root").expect("write the blocking root");
    let error = before_write(
        deliver_relative(&root, &root.join("out.bin"), "act-file", b"payload")
            .expect_err("a regular file is not a directory"),
    );
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(
        error.message.contains("open the export root"),
        "the refusal names the root open: {}",
        error.message
    );
    assert_eq!(
        std::fs::read(&root).expect("read the root file"),
        b"not-a-root",
        "the blocking file is untouched"
    );
}

/// A destination name too long to inspect fails the inspection before any
/// write, with the inspection's own error.
#[test]
fn an_uninspectable_destination_name_fails_at_the_inspection() {
    let scratch = Scratch::new();
    let (root, _outside) = root_and_outside(&scratch);
    let destination = root.join("x".repeat(300));
    let error = before_write(
        deliver_relative(&root, &destination, "act-long", b"payload")
            .expect_err("no filesystem accepts a 300-byte component"),
    );
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(
        error.message.contains("failed to inspect"),
        "the refusal names the inspection: {}",
        error.message
    );
    assert!(
        entries(&root).len() == 1,
        "only the real `nested` entry exists in the root"
    );
}

/// The scratch root, its real `nested` directory, and the outside directory the
/// swap would redirect a re-resolved link into.
struct Tree {
    root: std::path::PathBuf,
    outside: std::path::PathBuf,
    destination: std::path::PathBuf,
}

impl Tree {
    /// A scratch tree whose export root holds a real `nested` directory and
    /// whose `outside` directory sits beside the root, not inside it.
    fn new() -> (Scratch, Self) {
        let scratch = Scratch::new();
        let root = scratch.root.join("root");
        std::fs::create_dir_all(root.join("nested")).expect("create nested");
        let outside = scratch.root.join("outside");
        std::fs::create_dir_all(&outside).expect("create outside");
        let destination = root.join("nested").join("out.bin");
        (
            scratch,
            Self {
                root,
                outside,
                destination,
            },
        )
    }

    /// Swap the real `nested` directory for a link to `outside`: exactly the
    /// ancestor swap that redirected the old absolute-path link.
    fn swap_nested_for_a_link(&self) {
        std::fs::remove_dir(self.root.join("nested")).expect("remove the real nested");
        std::os::unix::fs::symlink(&self.outside, self.root.join("nested"))
            .expect("swap in the link");
    }

    /// Every entry in the outside directory.
    fn outside_entries(&self) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(&self.outside)
            .expect("read the outside directory")
            .map(|entry| entry.expect("dir entry").path())
            .collect()
    }
}

/// An ancestor swapped for a link after the confinement check is refused, and
/// nothing at all reaches the directory the link points at.
#[test]
fn an_ancestor_swap_is_refused_and_the_outside_directory_stays_empty() {
    let (_scratch, tree) = Tree::new();
    tree.swap_nested_for_a_link();
    let result = attempt_delivery(&tree.destination, "act-swap", b"payload", Some(&tree.root));
    assert!(
        matches!(result, Err(DeliveryFailure::BeforeWrite(_))),
        "the walk fails closed on the swapped ancestor"
    );
    assert!(
        tree.outside_entries().is_empty(),
        "no temp, no link, and nothing else is written outside"
    );
    assert!(!tree.outside.join("out.bin").exists());
}

/// The same tree without the swap delivers into the real nested directory and
/// leaves no temp sibling behind.
#[test]
fn a_nested_destination_with_a_trusted_root_lands_inside_it() {
    let (_scratch, tree) = Tree::new();
    let result = attempt_delivery(
        &tree.destination,
        "act-nested",
        b"payload",
        Some(&tree.root),
    );
    assert!(result.is_ok(), "the nested destination is written");
    assert_eq!(
        std::fs::read(&tree.destination).expect("read the destination"),
        b"payload"
    );
    assert!(
        std::fs::symlink_metadata(tree.root.join("nested"))
            .expect("inspect nested")
            .is_dir(),
        "the delivered directory is a real directory"
    );
    let siblings: Vec<std::path::PathBuf> = std::fs::read_dir(tree.root.join("nested"))
        .expect("read nested")
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    assert_eq!(
        siblings,
        vec![tree.destination.clone()],
        "the destination is the only entry: the temp sibling is gone"
    );
    assert!(tree.outside_entries().is_empty(), "nothing lands outside");
}
