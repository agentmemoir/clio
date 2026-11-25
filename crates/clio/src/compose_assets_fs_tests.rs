// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the held compose-root entry: the identity check after a name is
//! swapped, and the rewrite that refuses to claim a name it no longer holds.
//! All run in per-test temp directories; nothing outside them is touched.

use super::*;

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("am-compose-assets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp root");
    dir
}

#[test]
fn a_swapped_name_is_not_the_held_file_and_the_rewrite_refuses() {
    let root = temp_root("fs-swap");
    let shown = root.join("note.txt");
    std::fs::write(&shown, b"original").expect("write the held file");
    let opened = Root::open(&root).expect("open the root");
    let parent = opened
        .parent(Path::new(""))
        .expect("hold the root directory");
    let held = Root::hold(parent, OsStr::new("note.txt"), &shown).expect("hold the file");
    assert!(held.still_named(&shown).expect("identity check"));

    // Swap the name: the held object moves aside and a different file takes the name.
    let moved = root.join("moved.txt");
    std::fs::rename(&shown, &moved).expect("move the held file");
    std::fs::write(&shown, b"planted").expect("plant another file");

    assert!(
        !held.still_named(&shown).expect("identity check"),
        "the name must not resolve to the held object"
    );
    assert!(
        !held.rewrite(b"replacement", &shown).expect("rewrite"),
        "a rewrite must report that the name changed"
    );
    assert_eq!(
        std::fs::read(&shown).unwrap(),
        b"planted",
        "the planted file keeps its bytes"
    );
    assert_eq!(
        std::fs::read(&moved).unwrap(),
        b"replacement",
        "the write reached the held object, not the name"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_held_new_file_is_reported_as_created() {
    let root = temp_root("fs-created");
    let shown = root.join("new.txt");
    let opened = Root::open(&root).expect("open the root");
    let parent = opened
        .parent(Path::new(""))
        .expect("hold the root directory");
    let held = Root::hold(parent, OsStr::new("new.txt"), &shown).expect("create the file");
    assert!(held.was_created(), "a missing name is created by the hold");
    assert!(held.still_named(&shown).expect("identity check"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_parent_walk_creates_a_missing_component_and_refuses_a_parent_component() {
    let root = temp_root("fs-parent-walk");
    let opened = Root::open(&root).expect("open the root");
    opened
        .parent(Path::new("sql/nested"))
        .expect("create the components");
    assert!(
        root.join("sql/nested").is_dir(),
        "the walk creates directories"
    );
    let err = opened
        .parent(Path::new("../escape"))
        .expect_err("a parent component must be refused");
    assert!(err.contains(".."), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}
