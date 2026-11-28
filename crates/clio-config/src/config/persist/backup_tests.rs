// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the timestamped owner-only backup
//! taken before a deployment config is replaced, in `config::persist::backup`.
//!
//! ## Owns
//! - Backup location, name, permissions, and refusals under test
//!
//! ## Does not own
//! - Production backup logic under test
//! - The load and save sides of persistence (see `persist_tests`)
//!
//! ## Boundary
//!
//! Changes to backup naming, location, permissions, or what a backup refuses
//! should normally update tests here. Changes to the save path should not.
//!
//! Keep this module focused on the pre-replace backup.

use super::super::{Overlay, save_overlay_file};
use super::*;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

/// A unique scratch path under the system temp dir. Each test owns its own, so
/// no test reads or removes a directory another one created.
fn scratch(name: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-backup-{name}-{n}"))
}

#[test]
fn backup_reports_a_directory_creation_failure() {
    let dir = scratch("blocked");
    fs::create_dir_all(&dir).expect("dir");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, "{}").expect("write");
    // A file where the backups directory should be blocks create_dir_all.
    fs::write(dir.join(BACKUP_DIR_NAME), "not a directory").expect("blocker");
    let err = backup_config_file(&cfg).expect_err("mkdir");
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message.contains("failed to create backup dir"),
        "{}",
        err.message
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn backup_copies_owner_only_and_skips_missing() {
    let path = scratch("cfg/cfg.json");
    assert_eq!(backup_config_file(&path).expect("missing"), None);

    let mut overlay = Overlay::new();
    overlay.insert("backend".into(), json!("postgres"));
    save_overlay_file(&path, &overlay).expect("save");
    let backup = backup_config_file(&path).expect("backup").expect("some");
    assert!(backup.starts_with(path.parent().expect("parent").join(BACKUP_DIR_NAME)));
    assert!(
        backup.exists(),
        "the returned target must be the file the backup wrote"
    );
    assert_eq!(
        fs::read_to_string(&backup).expect("read"),
        fs::read_to_string(&path).expect("read")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&backup).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "backup must be owner-only");
    }
    // A directory source is refused by the descriptor's file-type check rather
    // than read, so nothing is backed up from a path that is not a file.
    let dir = scratch("dir");
    fs::create_dir_all(&dir).expect("dir");
    assert_eq!(
        backup_config_file(&dir).expect_err("read").code,
        ErrorCode::Forbidden
    );
    let _ = fs::remove_dir_all(&dir);
    if let Some(parent) = path.parent() {
        let _ = fs::remove_dir_all(parent);
    }
}

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

// The backup dir is created by the backup itself and is owner-only from the
// moment it exists, so the copied config is not left readable by other users of
// the machine.
#[cfg(unix)]
#[test]
fn backup_creates_a_owner_only_backups_dir() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("dir-mode");
    fs::create_dir_all(&dir).expect("dir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("mode");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, "{}").expect("write");

    backup_config_file(&cfg).expect("backup").expect("some");

    let backups = dir.join(BACKUP_DIR_NAME);
    let mode = fs::metadata(&backups).expect("meta").permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "the backups dir must be owner-only");
    let _ = fs::remove_dir_all(&dir);
}

// A backups directory the operator already made keeps the mode they chose: the
// backup creates a missing directory owner-only, it does not retighten one that
// exists.
#[cfg(unix)]
#[test]
fn backup_keeps_an_existing_backups_dir_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("dir-mode-existing");
    let backups = dir.join(BACKUP_DIR_NAME);
    fs::create_dir_all(&backups).expect("dir");
    fs::set_permissions(&backups, fs::Permissions::from_mode(0o750)).expect("mode");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, "{}").expect("write");

    backup_config_file(&cfg).expect("backup").expect("some");

    let mode = fs::metadata(&backups).expect("meta").permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o750,
        "an existing backups dir keeps the mode it had"
    );
    let _ = fs::remove_dir_all(&dir);
}

// A source the open cannot reach for a reason other than absence or a link, such
// as a parent that is not a directory, is an internal failure rather than a
// silent `None`: the caller must not be told there was nothing to protect.
#[test]
fn backup_reports_an_unopenable_source() {
    let dir = scratch("unopenable");
    fs::create_dir_all(&dir).expect("dir");
    let blocker = dir.join("blocker");
    fs::write(&blocker, "not a directory").expect("blocker");

    let err = backup_config_file(&blocker.join("deployment.json")).expect_err("open");

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message.contains("failed to read"),
        "the failure must name the read it could not do: {}",
        err.message
    );
    let _ = fs::remove_dir_all(&dir);
}

// Bytes that are not text cannot be copied into a backup that is later meant to
// be readable, so the backup fails instead of silently writing a lossy copy.
#[test]
fn backup_reports_a_source_that_is_not_text() {
    let dir = scratch("binary");
    fs::create_dir_all(&dir).expect("dir");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, [0xff, 0xfe, 0x00, 0x01]).expect("write");

    let err = backup_config_file(&cfg).expect_err("read");

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message.contains("failed to read"),
        "the failure must name the read it could not do: {}",
        err.message
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn backup_target_is_a_timestamped_namesake_in_the_backups_dir() {
    let dir = scratch("layout");
    fs::create_dir_all(&dir).expect("dir");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, r#"{"log.level":"warn"}"#).expect("write");
    let backups = dir.join(BACKUP_DIR_NAME);

    let first = backup_config_file(&cfg).expect("first").expect("some");
    let second = backup_config_file(&cfg).expect("second").expect("some");

    assert_eq!(
        first.parent(),
        Some(backups.as_path()),
        "the backup must sit in the sibling backups directory"
    );
    let name = first
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    let stamp = name
        .strip_prefix("deployment.json.")
        .unwrap_or_else(|| panic!("the backup name must carry the source name, got {name}"));
    assert!(
        stamp.parse::<u128>().is_ok_and(|n| n > 0),
        "the name must end in a nanosecond stamp, got {name}"
    );
    assert_ne!(
        first, second,
        "a second backup at the same stamp is refused, so two backups never share one name"
    );
    assert!(first.exists() && second.exists());
    assert_eq!(
        fs::read_dir(&backups).expect("read_dir").count(),
        2,
        "each backup is its own file"
    );
    let _ = fs::remove_dir_all(&dir);
}

// A source name long enough that the stamped target name exceeds the file-name
// limit leaves the source untouched and reports the write it could not do,
// instead of claiming a backup that is not there.
#[test]
fn backup_reports_a_target_name_that_cannot_be_written() {
    let dir = scratch("long-name");
    fs::create_dir_all(&dir).expect("dir");
    let cfg = dir.join(format!("{}.json", "d".repeat(240)));
    fs::write(&cfg, "{}").expect("write");

    let err = backup_config_file(&cfg).expect_err("write");

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message.contains("failed to write backup"),
        "the failure must name the backup it could not write: {}",
        err.message
    );
    assert_eq!(
        fs::read_to_string(&cfg).expect("source"),
        "{}",
        "a failed backup must not touch the source"
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
