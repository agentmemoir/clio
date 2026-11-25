// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Permission-refusal tests for the held compose-root entry. Root, and any
//! process holding `CAP_DAC_OVERRIDE`, bypasses the mode bits these cases rely
//! on, so each test probes the denied operation first: on a normal runner it
//! asserts the refusal, and on a privileged one it asserts the allowed outcome.
//! All run in per-test temp directories; nothing outside them is touched.

use super::*;

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("am-compose-assets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp root");
    dir
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("set the mode");
}

/// Whether the mode bits stop this process creating a file inside `dir`. The
/// probe asks the denied question directly and removes its file when allowed.
fn create_is_refused(dir: &Path) -> bool {
    let probe = dir.join(".create-probe");
    match std::fs::write(&probe, b"x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            false
        }
        Err(_) => true,
    }
}

/// Whether the mode bits stop this process listing `dir`.
fn read_dir_is_refused(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_err()
}

/// Whether the mode bits stop this process inspecting a name inside `dir`.
fn metadata_is_refused(path: &Path) -> bool {
    std::fs::metadata(path).is_err()
}

#[test]
fn a_leaf_that_cannot_be_created_is_reported() {
    let root = temp_root("fs-leaf-unwritable");
    let locked = root.join("locked");
    std::fs::create_dir(&locked).expect("create the directory");
    set_mode(&locked, 0o500);
    let refused = create_is_refused(&locked);
    let opened = Root::open(&root).expect("open the root");
    let parent = opened
        .parent(Path::new("locked"))
        .expect("hold the read-only directory");
    let shown = locked.join("new.sql");

    let outcome = Root::hold(parent, OsStr::new("new.sql"), &shown);
    if refused {
        let err = outcome
            .map(|_| ())
            .expect_err("a file cannot be created in a read-only directory");

        assert!(err.contains("cannot write"), "{err}");
        assert!(
            !shown.exists(),
            "the refused hold creates nothing at the asset name"
        );
    } else {
        let held = outcome.expect("a process that bypasses the mode bits creates the file");
        assert!(held.was_created(), "the held file is the one just created");
        assert!(
            held.read_all(&shown).expect("read the new file").is_empty(),
            "the created file holds no bytes yet"
        );
    }
    set_mode(&locked, 0o755);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_missing_parent_component_that_cannot_be_created_is_reported() {
    let root = temp_root("fs-parent-unwritable");
    let locked = root.join("locked");
    std::fs::create_dir(&locked).expect("create the directory");
    set_mode(&locked, 0o500);
    let refused = create_is_refused(&locked);
    let opened = Root::open(&root).expect("open the root");

    let outcome = opened.parent(Path::new("locked/sql"));
    if refused {
        let err = outcome
            .map(|_| ())
            .expect_err("a missing component cannot be created without write permission");

        assert!(err.contains("cannot create"), "{err}");
        assert!(
            !locked.join("sql").exists(),
            "the refused walk creates nothing behind the read-only directory"
        );
    } else {
        let walked = outcome.expect("a process that bypasses the mode bits creates the component");
        drop(walked);
        assert!(
            locked.join("sql").is_dir(),
            "the created component is a directory at that name"
        );
    }
    set_mode(&locked, 0o755);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_parent_component_that_cannot_be_read_is_refused() {
    let root = temp_root("fs-parent-unreadable");
    let locked = root.join("locked");
    std::fs::create_dir(&locked).expect("create the directory");
    set_mode(&locked, 0o300);
    let refused = read_dir_is_refused(&locked);
    let opened = Root::open(&root).expect("open the root");

    let outcome = opened.parent(Path::new("locked"));
    if refused {
        let err = outcome
            .map(|_| ())
            .expect_err("a directory that cannot be opened is refused");

        assert!(err.contains("cannot create"), "{err}");
    } else {
        outcome.expect("a process that bypasses the mode bits opens the directory");
        assert!(
            locked.is_dir(),
            "the opened component is the directory at that name"
        );
    }
    set_mode(&locked, 0o755);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn entry_type_reports_a_name_it_cannot_inspect() {
    let root = temp_root("fs-regular-unreadable");
    let locked = root.join("locked");
    std::fs::create_dir(&locked).expect("create the directory");
    std::fs::write(locked.join("001.sql"), b"select 1").expect("write the asset");
    set_mode(&locked, 0o400);
    let refused = metadata_is_refused(&locked.join("001.sql"));
    let opened = Root::open(&root).expect("open the root");

    let outcome = opened.entry_type(Path::new("locked/001.sql"));
    if refused {
        let err = outcome
            .expect_err("a name inside a directory with no search permission cannot be inspected");

        assert!(err.contains("cannot inspect"), "{err}");
    } else {
        assert_eq!(
            outcome.expect("a process that bypasses the search permission inspects the name"),
            Some(FileType::RegularFile),
            "the inspected entry is a regular file"
        );
    }
    set_mode(&locked, 0o755);
    let _ = std::fs::remove_dir_all(&root);
}
