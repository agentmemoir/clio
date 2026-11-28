// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the entries a backup refuses: a
//! symbolic link planted at the source, and a planted or linked entry at the
//! `backups/` directory or the timestamped target name.
//!
//! ## Owns
//! - Refusals for a symlink, directory, hard link, or dangling link at a name
//!   the backup reads or writes
//!
//! ## Does not own
//! - Production backup logic under test
//! - Names, permissions, and ordinary failures (see `backup_tests`)
//!
//! ## Boundary
//!
//! Changes to what a backup refuses belong here. Changes to its naming or
//! permissions should not.
//!
//! Keep this module focused on planted entries and link refusals.

use super::backup_tests::scratch;
use super::*;

// A symlink planted at the source is refused, so a backup never holds the bytes
// of a file the existence check approved. `O_NOFOLLOW` refuses the link in the
// one open, so no descriptor ever reaches the file behind it.
#[cfg(unix)]
#[test]
fn planted_symlink_at_backup_source_is_refused() {
    use std::os::unix::fs::symlink;
    let dir = scratch("symlink");
    fs::create_dir_all(&dir).expect("dir");
    let secret = r#"{"credentials.embed_api_key":"sk-keep-me"}"#;
    let victim = dir.join("victim.json");
    fs::write(&victim, secret).expect("victim");
    let source = dir.join("deployment.json");
    symlink(&victim, &source).expect("symlink");

    let err = backup_config_file(&source).expect_err("refused");

    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert!(
        err.message.contains("symbolic link"),
        "the refusal must name the link: {}",
        err.message
    );
    assert!(
        !dir.join(BACKUP_DIR_NAME).exists(),
        "a refused source must not leave a backup directory behind"
    );
    assert_eq!(
        fs::read_to_string(&victim).expect("victim"),
        secret,
        "the link target must be untouched"
    );
    assert!(
        fs::symlink_metadata(&source)
            .expect("meta")
            .file_type()
            .is_symlink(),
        "the planted entry must still be the symlink, not a replaced file"
    );
    let _ = fs::remove_dir_all(&dir);
}

// A symlink planted at the stamped backup name would be followed by an
// overwriting open, truncating the file it points at. The backup now creates its
// target exclusively, so the planted entry is refused and the link target keeps
// its bytes.
#[cfg(unix)]
#[test]
fn backup_refuses_a_planted_entry_at_the_target_name() {
    use std::os::unix::fs::symlink;
    let dir = scratch("planted-target");
    fs::create_dir_all(&dir).expect("dir");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, r#"{"log.level":"warn"}"#).expect("write");
    let secret = "bytes the link must keep";
    let victim = dir.join("victim.json");
    fs::write(&victim, secret).expect("victim");
    let backups = dir.join(BACKUP_DIR_NAME);
    fs::create_dir_all(&backups).expect("backups");
    let stamp = 42u128;
    let name = cfg
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    let target = backups.join(format!("{name}.{stamp}"));
    symlink(&victim, &target).expect("symlink");

    let err = backup_config_file_with_stamp(&cfg, stamp).expect_err("refused");

    assert_eq!(
        err.code,
        ErrorCode::Forbidden,
        "the refusal must surface as a refusal the operator can act on: {}",
        err.message
    );
    assert!(
        err.message.contains("already exists"),
        "the failure must name the entry that exists: {}",
        err.message
    );
    assert_eq!(
        fs::read_to_string(&victim).expect("victim"),
        secret,
        "the link target must keep its bytes"
    );
    assert!(
        fs::symlink_metadata(&target)
            .expect("meta")
            .file_type()
            .is_symlink(),
        "the planted entry must still be the symlink, not a replaced file"
    );
    let _ = fs::remove_dir_all(&dir);
}

// A directory planted at the stamped name is refused as an existing entry, so
// the backup neither writes into it nor replaces it.
#[cfg(unix)]
#[test]
fn backup_refuses_a_directory_at_the_target_name() {
    let dir = scratch("target-dir");
    fs::create_dir_all(dir.join(BACKUP_DIR_NAME)).expect("backups");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, "{}").expect("write");
    let target = dir.join(BACKUP_DIR_NAME).join("deployment.json.7");
    fs::create_dir(&target).expect("planted dir");

    let err = backup_config_file_with_stamp(&cfg, 7).expect_err("refused");

    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert!(fs::symlink_metadata(&target).expect("meta").is_dir());
    assert_eq!(fs::read_dir(&target).expect("read_dir").count(), 0);
    let _ = fs::remove_dir_all(&dir);
}

// A dangling symlink planted at the stamped name is not followed: the exclusive
// create refuses it, and the path it names is still absent.
#[cfg(unix)]
#[test]
fn backup_refuses_a_dangling_link_at_the_target_name() {
    use std::os::unix::fs::symlink;
    let dir = scratch("target-dangling");
    fs::create_dir_all(dir.join(BACKUP_DIR_NAME)).expect("backups");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, "{}").expect("write");
    let missing = dir.join("not-there.json");
    let target = dir.join(BACKUP_DIR_NAME).join("deployment.json.7");
    symlink(&missing, &target).expect("symlink");

    let err = backup_config_file_with_stamp(&cfg, 7).expect_err("refused");

    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    let entry = fs::symlink_metadata(&target).expect("meta");
    assert!(entry.file_type().is_symlink(), "still the planted link");
    assert_eq!(fs::read_link(&target).expect("read_link"), missing);
    assert!(!missing.exists(), "the link target must stay absent");
    let _ = fs::remove_dir_all(&dir);
}

// A hard link at the stamped name shares its bytes with another name, so an
// overwriting open would rewrite that file. The exclusive create refuses it and
// the shared bytes are unchanged.
#[cfg(unix)]
#[test]
fn backup_refuses_a_hard_link_at_the_target_name() {
    use std::os::unix::fs::MetadataExt;
    let dir = scratch("target-hard-link");
    fs::create_dir_all(dir.join(BACKUP_DIR_NAME)).expect("backups");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, "{}").expect("write");
    let secret = "bytes the other name must keep";
    let victim = dir.join("victim.json");
    fs::write(&victim, secret).expect("victim");
    let target = dir.join(BACKUP_DIR_NAME).join("deployment.json.7");
    fs::hard_link(&victim, &target).expect("hard link");

    let err = backup_config_file_with_stamp(&cfg, 7).expect_err("refused");

    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert_eq!(fs::read_to_string(&victim).expect("victim"), secret);
    let entry = fs::symlink_metadata(&target).expect("meta");
    assert!(entry.is_file(), "still the planted file");
    assert_eq!(entry.nlink(), 2, "the two names must still share an inode");
    let _ = fs::remove_dir_all(&dir);
}

// A symlink planted at the backups directory name is refused with Forbidden.
// The backup inspects the entry before any create, so no directory is made and
// nothing lands in the directory the link points at.
#[cfg(unix)]
#[test]
fn backup_refuses_a_symlinked_backups_directory() {
    use std::os::unix::fs::symlink;
    let dir = scratch("symlink-dir");
    fs::create_dir_all(&dir).expect("dir");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, r#"{"log.level":"warn"}"#).expect("write");
    let target_dir = scratch("symlink-dir-target");
    fs::create_dir_all(&target_dir).expect("target dir");
    let backups = dir.join(BACKUP_DIR_NAME);
    symlink(&target_dir, &backups).expect("symlink");

    let err = backup_config_file(&cfg).expect_err("refused");

    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert!(
        err.message.contains("symbolic link"),
        "the refusal must name the link: {}",
        err.message
    );
    assert_eq!(
        fs::read_dir(&target_dir).expect("read_dir").count(),
        0,
        "a refused backups dir must not receive a backup file"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&target_dir);
}
