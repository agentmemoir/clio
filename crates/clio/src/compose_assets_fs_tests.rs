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

#[test]
fn a_root_that_is_not_a_directory_is_reported() {
    let dir = temp_root("fs-root-is-file");
    let file = dir.join("compose-root");
    std::fs::write(&file, b"not a directory").expect("plant a file at the root path");
    let err = Root::open(&file)
        .map(|_| ())
        .expect_err("a file cannot be the compose root");
    assert!(err.contains("cannot create"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cur_dir_component_is_skipped_by_the_walk() {
    let root = temp_root("fs-cur-dir");
    let opened = Root::open(&root).expect("open the root");

    opened
        .parent(Path::new("./sql"))
        .expect("a dot component names the held root itself");

    assert!(
        root.join("sql").is_dir(),
        "the walk creates the component below the held root"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_missing_parent_chain_is_created_and_a_second_walk_reuses_it() {
    let root = temp_root("fs-parent-reuse");
    let opened = Root::open(&root).expect("open the root");
    let first = opened
        .parent(Path::new("sql/one"))
        .expect("create the chain");
    drop(first);
    let marker = root.join("sql/one/marker.txt");
    std::fs::write(&marker, b"kept").expect("write into the reached directory");
    let second = opened
        .parent(Path::new("sql/one"))
        .expect("reuse the existing chain");
    drop(second);
    assert_eq!(
        std::fs::read(&marker).expect("read the marker"),
        b"kept",
        "the second walk reuses the directory the first one created"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_symlinked_parent_component_is_refused_and_not_followed() {
    let root = temp_root("fs-parent-symlink");
    let outside = root.join("outside");
    std::fs::create_dir(&outside).expect("create the target directory");
    std::os::unix::fs::symlink(&outside, root.join("linked")).expect("plant the symlink");
    let opened = Root::open(&root).expect("open the root");

    let err = opened
        .parent(Path::new("linked/one"))
        .expect_err("a symlinked component must be refused");

    assert!(err.contains("linked"), "{err}");
    assert!(
        !outside.join("one").exists(),
        "the walk must not follow the symlink and create the component behind it"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_file_at_a_parent_component_is_refused_as_not_a_directory() {
    let root = temp_root("fs-parent-file");
    std::fs::write(root.join("sql"), b"not a directory").expect("plant the file");
    let opened = Root::open(&root).expect("open the root");
    let err = opened
        .parent(Path::new("sql"))
        .expect_err("a plain file cannot be walked into");
    assert!(err.contains("it is not a directory"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_absent_root_is_reported() {
    let missing =
        std::env::temp_dir().join(format!("am-compose-assets-absent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    let err = Root::open(&missing)
        .map(|_| ())
        .expect_err("a missing root must be refused");
    assert!(err.contains("cannot create"), "{err}");
}

#[test]
fn a_directory_at_the_asset_name_is_refused() {
    let root = temp_root("fs-leaf-directory");
    let shown = root.join("note.txt");
    std::fs::create_dir(&shown).expect("plant a directory at the asset name");
    let opened = Root::open(&root).expect("open the root");
    let parent = opened
        .parent(Path::new(""))
        .expect("hold the root directory");
    let err = Root::hold(parent, OsStr::new("note.txt"), &shown)
        .map(|_| ())
        .expect_err("a directory cannot be written as a file");
    assert!(err.contains("cannot write"), "{err}");
    assert!(shown.is_dir(), "the refused directory is left in place");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_named_pipe_at_the_asset_name_is_refused_as_not_a_regular_file() {
    let root = temp_root("fs-leaf-pipe");
    let shown = root.join("note.txt");
    let opened = Root::open(&root).expect("open the root");
    let parent = opened
        .parent(Path::new(""))
        .expect("hold the root directory");
    rustix::fs::mknodat(
        &parent,
        "note.txt",
        FileType::Fifo,
        Mode::from(0o600),
        rustix::fs::Dev::from(0_u64),
    )
    .expect("plant a named pipe at the asset name");
    let err = Root::hold(parent, OsStr::new("note.txt"), &shown)
        .map(|_| ())
        .expect_err("a named pipe is not a regular file");
    assert!(err.contains("it is not a regular file"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_backup_name_already_taken_takes_the_next_free_suffix() {
    let root = temp_root("fs-backup-collision");
    let shown = root.join("note.txt");
    std::fs::write(&shown, b"edited by hand").expect("write the held file");
    let planted = root.join("note.txt.bak-7");
    std::fs::write(&planted, b"older backup").expect("plant the taken backup name");
    let opened = Root::open(&root).expect("open the root");
    let parent = opened
        .parent(Path::new(""))
        .expect("hold the root directory");
    let held = Root::hold(parent, OsStr::new("note.txt"), &shown).expect("hold the file");
    let bytes = held.read_all(&shown).expect("read the held bytes");

    let created = held.backup(&bytes, 7, &shown).expect("create the backup");

    assert_eq!(created, OsStr::new("note.txt.bak-7-1"));
    assert_eq!(
        std::fs::read(&planted).expect("read the planted backup"),
        b"older backup",
        "the taken name keeps its bytes"
    );
    assert_eq!(
        std::fs::read(root.join("note.txt.bak-7-1")).expect("read the new backup"),
        b"edited by hand",
        "the next free name holds the previous content"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_backup_name_keeps_the_extension_and_appends_the_stamp() {
    assert_eq!(
        backup_name(Path::new("sql/001_slots.sql"), 7),
        Path::new("sql/001_slots.sql.bak-7")
    );
    assert_eq!(
        backup_name(Path::new("compose.yaml"), 0),
        Path::new("compose.yaml.bak-0")
    );
    assert_eq!(
        backup_name(Path::new("models/README.md"), 7),
        Path::new("models/README.md.bak-7")
    );
}

#[test]
fn split_refuses_a_path_with_no_file_name() {
    let err = split(Path::new("..")).expect_err("a parent path has no leaf name");
    assert!(err.contains("is not a compose asset file path"), "{err}");
}

#[test]
fn read_all_returns_the_whole_file_across_reads() {
    let root = temp_root("fs-read-all");
    let shown = root.join("schema.sql");
    let content: Vec<u8> = (0..20_000_u32).map(|n| (n % 251) as u8).collect();
    std::fs::write(&shown, &content).expect("write a file larger than the read buffer");
    let opened = Root::open(&root).expect("open the root");
    let parent = opened
        .parent(Path::new(""))
        .expect("hold the root directory");
    let held = Root::hold(parent, OsStr::new("schema.sql"), &shown).expect("hold the file");

    let bytes = held.read_all(&shown).expect("read every byte");

    assert_eq!(
        bytes, content,
        "every byte is read, not just the first chunk"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_removed_name_is_not_the_held_file() {
    let root = temp_root("fs-removed");
    let shown = root.join("note.txt");
    std::fs::write(&shown, b"original").expect("write the held file");
    let opened = Root::open(&root).expect("open the root");
    let parent = opened
        .parent(Path::new(""))
        .expect("hold the root directory");
    let held = Root::hold(parent, OsStr::new("note.txt"), &shown).expect("hold the file");
    std::fs::remove_file(&shown).expect("remove the name");

    assert!(
        !held.still_named(&shown).expect("identity check"),
        "a name that no longer exists is not the held file"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn entry_type_reports_a_missing_name_as_absent() {
    let root = temp_root("fs-regular-missing");
    std::fs::write(root.join("present.sql"), b"select 1").expect("write the asset");
    let opened = Root::open(&root).expect("open the root");

    assert_eq!(
        opened.entry_type(Path::new("present.sql")).expect("check"),
        Some(FileType::RegularFile)
    );
    assert_eq!(
        opened.entry_type(Path::new("absent.sql")).expect("check"),
        None,
        "a missing name has no entry"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn entry_type_names_a_directory_and_a_symlink() {
    let root = temp_root("fs-regular-refuse");
    std::fs::create_dir(root.join("models")).expect("plant a directory");
    std::fs::write(root.join("real.sql"), b"select 1").expect("write the target file");
    std::os::unix::fs::symlink(root.join("real.sql"), root.join("linked.sql"))
        .expect("plant the symlink");
    let opened = Root::open(&root).expect("open the root");

    assert_eq!(
        opened.entry_type(Path::new("models")).expect("check"),
        Some(FileType::Directory),
        "a directory is not a regular file"
    );
    assert_eq!(
        opened.entry_type(Path::new("linked.sql")).expect("check"),
        Some(FileType::Symlink),
        "a symlink is inspected without being followed"
    );
    let _ = std::fs::remove_dir_all(&root);
}
