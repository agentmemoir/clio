// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the descriptor-relative delivery: that an ancestor swapped
//! for a link between the confinement check and the link cannot place the
//! bundle outside the export root, and that a nested destination under a
//! trusted root still lands inside the real directory it names.
//!
//! ## Owns
//! - The swap regression: the refused delivery and the untouched outside
//!   directory
//! - The positive control: the same tree without the swap still delivers
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

use crate::export_custody::DeliveryFailure;
use crate::export_custody::attempt_delivery;
use crate::export_custody::export_custody_tests::Scratch;

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
