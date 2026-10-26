// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MemTree attach tests: the parent moves a
//! fan-out split makes and how long an attach reports them.
//!
//! ## Owns
//! - The children a split re-parents, and the two halves a new root takes
//! - That those moves belong to one attach and the next attach forgets them
//!
//! ## Does not own
//! - The rows those moves become (`memtree_hook_tests`)
//! - Dirty-path refresh, retrieval, or tool handlers (`memtree_tests`)
//!
//! ## Boundary
//!
//! Changes to what an attach records for persistence should normally require changes here.
//! Changes to the rows a persist writes should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use crate::memtree::{AttachLeaf, BankMemTree};

const BANK: &str = "attach-bank";
/// Small enough that four leaves overflow the root and two more overflow a half.
const FANOUT: usize = 3;

fn leaf(item_id: &str, at: &str) -> AttachLeaf {
    AttachLeaf {
        item_id: item_id.into(),
        bank_id: BANK.into(),
        interval_start: at.into(),
        interval_end: at.into(),
        gist: None,
        task_id: None,
    }
}

/// A tree holding `items` oldest first, on a fixed synthetic time line.
fn tree_of(items: &[&str]) -> BankMemTree {
    let mut tree = BankMemTree::new(BANK, FANOUT);
    for (n, item) in items.iter().enumerate() {
        let at = format!("{:02}", 10 * (n + 1));
        tree.attach_leaf(leaf(item, &at))
            .expect("attach in time order");
    }
    tree
}

fn placed(tree: &BankMemTree, items: &[&str]) -> Vec<String> {
    items
        .iter()
        .map(|i| {
            tree.leaf_node_id_for_item(i)
                .unwrap_or_else(|| panic!("{i} was placed"))
                .to_owned()
        })
        .collect()
}

#[test]
fn a_split_records_the_children_it_moved_under_the_new_sibling() {
    // Four leaves fill the root, so the fourth splits it and the root grows a
    // level. The next two land on the right half and overflow only that node,
    // which is the split whose moves the persist path used to miss.
    let mut tree = tree_of(&["a", "b", "c", "d"]);
    tree.attach_leaf(leaf("e", "50")).expect("attach e");
    tree.attach_leaf(leaf("f", "60")).expect("attach f");

    let moved = placed(&tree, &["e", "f"]);
    let mut reported = tree.reparented.clone();
    reported.sort();
    let mut expected = moved.clone();
    expected.sort();
    assert_eq!(reported, expected, "exactly the moved children are pending");

    let sibling_id = tree
        .get(&moved[0])
        .and_then(|n| n.parent_id.clone())
        .expect("the moved child has a parent");
    assert_eq!(
        tree.get(&moved[1]).and_then(|n| n.parent_id.clone()),
        Some(sibling_id.clone()),
        "both moved children share the parent the split created"
    );
    let sibling = tree.get(&sibling_id).expect("sibling node");
    assert_eq!(
        sibling.child_ids, moved,
        "the sibling holds the moved children in time order"
    );
    assert_eq!(
        sibling.parent_id,
        tree.root_id().map(str::to_owned),
        "the sibling hangs under the existing root"
    );
}

#[test]
fn the_next_attach_forgets_the_moves_of_the_previous_one() {
    let mut tree = tree_of(&["a", "b", "c", "d"]);
    tree.attach_leaf(leaf("e", "50")).expect("attach e");
    tree.attach_leaf(leaf("f", "60")).expect("attach f");
    assert!(
        !tree.reparented.is_empty(),
        "the overflowing attach left its moves pending"
    );

    let (_, dirty) = tree.attach_leaf(leaf("g", "70")).expect("attach g");

    assert!(
        tree.reparented.is_empty(),
        "the last attach split nothing, so no earlier move is reported again: {:?}",
        tree.reparented
    );
    assert!(
        !dirty.is_empty(),
        "the attach still names the ancestors it dirtied: {dirty:?}"
    );
}

#[test]
fn a_split_that_grows_the_root_records_both_halves() {
    let tree = tree_of(&["a", "b", "c", "d"]);
    let root_id = tree.root_id().expect("a root after four leaves").to_owned();
    let root = tree.get(&root_id).expect("root node");
    assert_eq!(
        root.child_ids.len(),
        2,
        "the overflowed root became two nodes"
    );

    for half in &root.child_ids {
        assert!(
            tree.reparented.contains(half),
            "the new root took {half}, so its parent is new and pending"
        );
        assert_eq!(
            tree.get(half).and_then(|n| n.parent_id.clone()),
            Some(root_id.clone()),
            "{half} hangs under the root the split created"
        );
    }

    let right = tree
        .get(&root.child_ids[1])
        .expect("right half")
        .child_ids
        .clone();
    assert_eq!(right.len(), 2, "the right half took the later children");
    for child in &right {
        assert!(
            tree.reparented.contains(child),
            "the child the split moved, {child}, is pending"
        );
    }
}
