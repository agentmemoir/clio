// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of owner-only file writes and the
//! owner-only directory rule in `config::persist::write`.
//!
//! ## Owns
//! - Write behavior under test for `config::persist::write`
//!
//! ## Does not own
//! - Production write logic under test
//! - What is written into the file, or the backup layout
//!
//! ## Boundary
//!
//! Changes to how bytes reach disk or to the owner-only rule should normally
//! update tests here. Changes to overlay contents should not.
//!
//! Keep this module focused on owner-only write tests.

use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn tmp(name: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-write-{name}-{n}"))
}

/// Sorted entry names in `dir`, so the assertion never depends on `read_dir`
/// order.
fn entry_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn atomic_write_lands_owner_only_contents_without_a_temp_file() {
    let dir = tmp("ok");
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("deployment.json");

    write_private_atomic(&path, "hello").expect("write");

    assert_eq!(fs::read_to_string(&path).expect("read"), "hello");
    assert_eq!(
        entry_names(&dir),
        vec!["deployment.json".to_owned()],
        "the renamed temp file must not outlive the write"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "config must be owner-only");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn write_private_replaces_an_existing_file_and_forces_owner_only() {
    let dir = tmp("replace");
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("backup.json");
    fs::write(&path, "old").expect("seed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode");
    }

    write_private(&path, "new").expect("write");

    assert_eq!(fs::read_to_string(&path).expect("read"), "new");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a replaced file must be owner-only, not keep its old mode"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn temp_sibling_stays_beside_the_target_and_is_unique() {
    let path = Path::new("/tmp").join("deployment.json");

    let first = temp_sibling(&path);
    let second = temp_sibling(&path);

    assert_eq!(
        first.parent(),
        path.parent(),
        "the temp file must stay on the target's filesystem"
    );
    let name = first
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with(".deployment.json.")
            && Path::new(&name).extension() == Some(std::ffi::OsStr::new("tmp")),
        "temp name must be hidden, carry the target name, and end in .tmp, got {name}"
    );
    assert_ne!(first, second, "concurrent temp files must be distinct");
    // A path with no file name falls back to the deployment file name.
    let fallback = temp_sibling(Path::new("/"));
    assert!(
        fallback
            .file_name()
            .expect("name")
            .to_string_lossy()
            .contains(DEPLOYMENT_CONFIG_FILE_NAME),
        "a nameless target uses the deployment file name, got {fallback:?}"
    );
}

#[test]
fn atomic_write_removes_the_temp_file_when_the_rename_fails() {
    let dir = tmp("rename-fail");
    fs::create_dir_all(&dir).expect("dir");
    // A non-empty directory at the target name cannot be replaced by a rename.
    let path = dir.join("deployment.json");
    fs::create_dir_all(&path).expect("target dir");
    fs::write(path.join("child"), "x").expect("child");

    write_private_atomic(&path, "hello").expect_err("rename must fail");

    assert_eq!(
        entry_names(&dir),
        vec!["deployment.json".to_owned()],
        "a failed rename must remove its own temp file"
    );
    assert_eq!(
        fs::read_to_string(path.join("child")).expect("child"),
        "x",
        "the failed write must not touch the existing target"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn restrict_dir_sets_owner_only_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp("restrict");
    fs::create_dir_all(&dir).expect("dir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("mode");

    restrict_dir(&dir);

    let mode = fs::metadata(&dir).expect("meta").permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "config dir must be owner-only");
    let _ = fs::remove_dir_all(&dir);
}

// A symlink planted at the temp name is refused, so the file it points at is
// never truncated. `create_new` is `O_CREAT|O_EXCL`, which fails with
// `AlreadyExists` on an existing final component, a symlink included.
#[cfg(unix)]
#[test]
fn planted_symlink_at_temp_name_is_refused() {
    use std::os::unix::fs::symlink;
    let dir = tmp("symlink-plant");
    fs::create_dir_all(&dir).expect("dir");
    let victim = dir.join("victim.json");
    fs::write(&victim, "keep me").expect("victim");
    let planted = dir.join(".deployment.json.0.0.0.tmp");
    symlink(&victim, &planted).expect("symlink");

    let err = create_new_private(&planted, "overwrite").expect_err("refused");
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&victim).expect("victim"), "keep me");
    assert!(
        fs::symlink_metadata(&planted)
            .expect("meta")
            .file_type()
            .is_symlink(),
        "the planted entry must still be the symlink, not a replaced file"
    );
    let _ = fs::remove_dir_all(&dir);
}
