// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Regression tests for a compose-root writer that swaps an asset, or a parent
//! component, between the steps that read, back up, and replace it. Each case
//! must be refused, and nothing outside the root may change.

use super::*;
use crate::compose_assets_fs::backup_name;

fn temp_root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("am-compose-assets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp root");
    dir
}

fn backup_paths(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut names: Vec<_> = std::fs::read_dir(root)
        .expect("read the root")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with("compose.yml.bak-"))
        .collect();
    names.sort();
    names.into_iter().map(|name| root.join(name)).collect()
}

#[cfg(unix)]
#[test]
fn materialize_refuses_an_asset_that_is_a_symlink_outside_the_root() {
    let root = temp_root("race-leaf");
    let outside = temp_root("race-leaf-outside");
    let victim = outside.join("victim");
    std::fs::write(&victim, "outside bytes").expect("write the outside file");
    std::os::unix::fs::symlink(&victim, root.join("compose.yml")).expect("plant the symlink");

    let err =
        materialize(&root, ComposeTarget::MacArm64).expect_err("a symlinked asset must be refused");
    assert!(err.contains("compose.yml"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "outside bytes",
        "the file outside the root must not be touched"
    );
    assert!(
        !root.join("compose.arm64.yml").exists(),
        "the run stops at the refused asset"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

#[cfg(unix)]
#[test]
fn materialize_refuses_a_parent_component_that_is_a_symlink_outside_the_root() {
    let root = temp_root("race-parent");
    let outside = temp_root("race-parent-outside");
    std::os::unix::fs::symlink(&outside, root.join("sql")).expect("plant the directory symlink");

    let err = materialize(&root, ComposeTarget::MacArm64)
        .expect_err("a symlinked parent must be refused");
    assert!(err.contains("sql"), "{err}");
    assert!(
        std::fs::read_dir(&outside).unwrap().next().is_none(),
        "nothing may be created outside the root"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn a_planted_backup_name_is_not_overwritten() {
    let root = temp_root("race-backup-name");
    let shown = root.join("compose.yml");
    std::fs::write(&shown, b"edited").expect("write the edited asset");
    let stamp = 7;
    let planted = backup_name(&shown, stamp);
    std::fs::write(&planted, b"planted").expect("plant the backup name");

    let opened = Root::open(&root).expect("open the root");
    let parent = opened
        .parent(Path::new(""))
        .expect("hold the root directory");
    let held =
        Root::hold(parent, std::ffi::OsStr::new("compose.yml"), &shown).expect("hold the asset");
    let read = held.read_all(&shown).expect("read the asset");
    let backup = held.backup(&read, stamp, &shown).expect("create a backup");

    assert_eq!(
        backup,
        std::ffi::OsString::from("compose.yml.bak-7-1"),
        "an occupied name takes the next free one"
    );
    assert_eq!(
        std::fs::read(&planted).unwrap(),
        b"planted",
        "the planted entry keeps its bytes"
    );
    assert_eq!(
        std::fs::read(root.join("compose.yml.bak-7-1")).unwrap(),
        b"edited",
        "the backup holds the bytes that were read"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn bind_mount_check_refuses_a_symlinked_source() {
    let root = temp_root("race-mount-symlink");
    let outside = temp_root("race-mount-outside");
    let victim = outside.join("001_core.sql");
    std::fs::write(&victim, "outside bytes").expect("write the outside file");
    materialize(&root, ComposeTarget::MacArm64).expect("materialize");
    let source = root.join("sql/001_core.sql");
    std::fs::remove_file(&source).expect("remove the materialized source");
    std::os::unix::fs::symlink(&victim, &source).expect("plant the symlink");

    let err = verify_bind_mounts(&root).expect_err("a symlinked source must fail");
    assert!(err.contains("sql/001_core.sql"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn an_edited_asset_backup_holds_the_exact_bytes_that_were_read() {
    let root = temp_root("race-backup-bytes");
    materialize(&root, ComposeTarget::MacArm64).expect("first materialize");
    let edited: &[u8] = b"\xff\x00hand edit\n";
    std::fs::write(root.join("compose.yml"), edited).expect("edit the asset");

    materialize(&root, ComposeTarget::MacArm64).expect("second materialize");
    let backups = backup_paths(&root);
    assert_eq!(backups.len(), 1, "one backup: {backups:?}");
    assert_eq!(
        std::fs::read(&backups[0]).unwrap(),
        edited,
        "the backup is the exact byte sequence that was read"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn a_root_reached_through_a_symlink_still_materializes() {
    let real = temp_root("race-symlink-root-real");
    let holder = temp_root("race-symlink-root-holder");
    let linked = holder.join("root");
    std::os::unix::fs::symlink(&real, &linked).expect("plant the root symlink");

    materialize(&linked, ComposeTarget::MacArm64).expect("materialize through the symlink");
    assert!(
        real.join("compose.yml").is_file(),
        "the asset lands in the real root"
    );
    let _ = std::fs::remove_dir_all(&real);
    let _ = std::fs::remove_dir_all(&holder);
}
