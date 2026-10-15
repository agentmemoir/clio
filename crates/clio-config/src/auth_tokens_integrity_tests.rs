// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the token store's harder guarantees: that the path
//! resolver's blank-override reading matches its recorded decision, that a
//! write touches only the permissions it chose, that a hand-edited record is
//! refused by the same validator the writer uses, and that a mutation is
//! atomic against a second writer.
//!
//! ## Owns
//! - Behavior tests for path resolution, parent-directory modes, loaded-record
//!   validation, and the locked add/revoke composition
//!
//! ## Does not own
//! - The store implementation under test (`auth_tokens`)
//! - The record round-trip and digest tests (they live in `auth_tokens_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

// Both test modules are declared with `#[path]`, so they sit beside each other
// under `auth_tokens` rather than nested inside one another; this is the
// sibling path to the shared fixtures.
use super::*;
use crate::auth_tokens::tests::{lookup, sample_record};
use std::fs;

/// Unique scratch path for this module, named like the other token-store tests
/// so a reader recognizes it, and unique per call so parallel tests never
/// share a store or its lock file.
///
/// A scenario that needs its own directory takes `scratch(name)` as that
/// directory. No caller rewrites the returned path with `with_extension`,
/// because that replaces the nanosecond stamp and two runs would land on the
/// same path — which is how this module's first version leaked files into
/// /tmp between runs.
fn scratch(name: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-auth-integrity-{name}-{n}"))
}

#[test]
fn path_resolution_prefers_the_override_then_the_xdg_default() {
    let explicit = PathBuf::from("/tmp/explicit-store.json");
    assert_eq!(
        resolve_token_store_path_with(Some(explicit.clone()), lookup(&[])),
        Some(explicit)
    );
    assert_eq!(
        resolve_token_store_path_with(None, lookup(&[(AUTH_TOKEN_FILE_ENV, "/tmp/via-env.json")])),
        Some(PathBuf::from("/tmp/via-env.json"))
    );
    // A blank override counts as unset, matching every other env override, so
    // it falls through to the XDG default rather than refusing. The recorded
    // Decision Gates row says the same, and the CLI caller now agrees.
    assert_eq!(
        resolve_token_store_path_with(
            None,
            lookup(&[
                (AUTH_TOKEN_FILE_ENV, "  "),
                ("XDG_CONFIG_HOME", "/xdg"),
                ("HOME", "/home/op")
            ])
        ),
        Some(PathBuf::from("/xdg/clio/").join(AUTH_TOKEN_FILE_NAME))
    );
    // XDG base wins over HOME; a relative XDG base is ignored.
    assert_eq!(
        resolve_token_store_path_with(
            None,
            lookup(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/op")])
        ),
        Some(PathBuf::from("/xdg/clio/").join(AUTH_TOKEN_FILE_NAME))
    );
    assert_eq!(
        resolve_token_store_path_with(
            None,
            lookup(&[("XDG_CONFIG_HOME", "relative"), ("HOME", "/home/op")])
        ),
        Some(PathBuf::from("/home/op/.config/clio/").join(AUTH_TOKEN_FILE_NAME))
    );
    // A blank override with no config directory at all is still unresolvable.
    assert_eq!(
        resolve_token_store_path_with(None, lookup(&[(AUTH_TOKEN_FILE_ENV, "")])),
        None
    );
    assert_eq!(resolve_token_store_path_with(None, lookup(&[])), None);
}

#[test]
fn a_parent_directory_this_call_created_is_owner_only() {
    let dir = scratch("fresh-parent");
    let path = dir.join(AUTH_TOKEN_FILE_NAME);
    save_auth_tokens(&path, &[sample_record("a")]).expect("save creates its parent");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dir).expect("meta").permissions().mode();
        assert_eq!(mode & 0o77, 0, "a created parent is owner-only: {mode:o}");
    }
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn a_parent_directory_the_caller_already_had_keeps_its_mode() {
    // A store written beside files the store knows nothing about must not
    // tighten their directory: that changes what every other file in it can
    // be read by, which is a side effect the deployment-config writer does not
    // have either. The file's own owner-only mode is what the load-time
    // refusal checks, and that is still enforced.
    let dir = scratch("kept-parent");
    fs::create_dir_all(&dir).expect("create");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o750)).expect("set");
    }
    let path = dir.join(AUTH_TOKEN_FILE_NAME);
    save_auth_tokens(&path, &[sample_record("a")]).expect("save");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dir).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o750, "the parent keeps its mode: {mode:o}");
        // And the store itself is still owner-only, which is what is enforced.
        let file_mode = fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(file_mode & 0o77, 0, "the file is owner-only: {file_mode:o}");
    }
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn a_hand_edited_bank_entry_is_refused_even_though_the_list_is_not_empty() {
    // The writer refuses each of these through `validate_banks`; the loader
    // has to refuse the same values, or the two halves of one store disagree
    // about what a valid record is and a downstream transport inherits the gap.
    for banks in [
        vec![String::new()],
        vec!["  ".to_owned()],
        vec!["demo".into(), "demo".into()],
    ] {
        let mut record = sample_record("hand-edited");
        record.banks = banks.clone();
        let path = scratch("hand-edited-banks");
        save_auth_tokens(&path, &[record]).expect("save writes what it is told");
        let err = load_auth_tokens(&path).expect_err(&format!("banks {banks:?}"));
        assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{banks:?}: {err}");
        let _ = fs::remove_file(&path);
    }
    // A repeated bank that differs only by padding is the same repeated entry.
    let mut record = sample_record("padded");
    record.banks = vec!["demo".into(), " demo ".into()];
    let path = scratch("padded-banks");
    save_auth_tokens(&path, &[record]).expect("save");
    assert_eq!(
        load_auth_tokens(&path).expect_err("padded duplicate").code,
        ErrorCode::ConfigCorrupt
    );
    let _ = fs::remove_file(&path);
    // And the ordinary case still loads.
    let path = scratch("clean-banks");
    save_auth_tokens(&path, &[sample_record("clean")]).expect("save");
    assert_eq!(load_auth_tokens(&path).expect("clean banks").len(), 1);
    let _ = fs::remove_file(&path);
}

#[test]
fn a_created_at_that_is_free_text_is_refused() {
    // `created_at` is the one field a downstream server shows, so a
    // hand-edited value must not be able to carry anything else.
    for stamp in [
        "",
        "yesterday",
        "2026-10-01",
        "2026-13-01T00:00:00Z",
        "2026-10-01T24:00:00Z",
    ] {
        let mut record = sample_record("bad-stamp");
        record.created_at = stamp.to_owned();
        let path = scratch("bad-stamp");
        save_auth_tokens(&path, &[record]).expect("save");
        let err = load_auth_tokens(&path).expect_err(&format!("stamp {stamp:?}"));
        assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{stamp:?}: {err}");
        let _ = fs::remove_file(&path);
    }
    // The shape the writer produces is accepted, and a leap day parses.
    assert!(valid_created_at("2026-10-01T00:00:00Z"));
    assert!(valid_created_at("2024-02-29T23:59:59Z"));
    assert!(
        !valid_created_at("2024-02-30T00:00:00Z"),
        "day range is checked"
    );
}

#[test]
fn add_token_and_revoke_token_go_through_the_locked_store() {
    // The composition the CLI uses: both take the exclusive hold for their
    // whole read-modify-write, so a concurrent writer cannot lose a record.
    let path = scratch("transactional");
    let (first, secret) =
        add_token(&path, "one", "agent", &["demo".to_owned()]).expect("first add");
    assert_eq!(first.label, "one");
    assert_eq!(first.secret_sha256, digest_hex(&secret));
    let (second, _) = add_token(&path, "two", "user", &["demo".to_owned()]).expect("second add");
    assert_eq!(second.label, "two");
    // Both survive: the second add read a store that already held the first.
    let labels: Vec<String> = load_auth_tokens(&path)
        .expect("load")
        .into_iter()
        .map(|r| r.label)
        .collect();
    assert_eq!(labels, vec!["one".to_owned(), "two".to_owned()]);

    // A taken label is refused and nothing is written.
    let bytes_before = fs::read_to_string(&path).expect("bytes");
    let err = add_token(&path, "one", "agent", &["demo".to_owned()]).expect_err("taken");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("already exists"), "{err}");
    assert_eq!(fs::read_to_string(&path).expect("bytes"), bytes_before);

    // Bad input is refused before any file is touched.
    assert!(add_token(&path, "bad label", "agent", &["demo".to_owned()]).is_err());
    assert!(add_token(&path, "three", "robot", &["demo".to_owned()]).is_err());
    assert!(add_token(&path, "three", "agent", &[String::new()]).is_err());
    assert_eq!(fs::read_to_string(&path).expect("bytes"), bytes_before);

    revoke_token(&path, "one").expect("revoke");
    let left: Vec<String> = load_auth_tokens(&path)
        .expect("load")
        .into_iter()
        .map(|r| r.label)
        .collect();
    assert_eq!(left, vec!["two".to_owned()]);
    let err = revoke_token(&path, "one").expect_err("already gone");
    assert_eq!(err.code, ErrorCode::NotFound);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(lock_file_path(&path));
}

#[test]
fn a_store_whose_parent_is_a_file_refuses_every_write() {
    // Each failure arm is produced the way an operator hits it, not by mocking
    // the filesystem: a regular file where the store's parent directory belongs
    // makes the directory create, the lock open, and the write all fail for
    // real, and each refusal names the step that failed.
    let blocker = scratch("parent-is-a-file");
    fs::write(&blocker, b"not a directory").expect("write the blocking file");
    let store = blocker.join(AUTH_TOKEN_FILE_NAME);

    let err =
        save_auth_tokens(&store, &[sample_record("a")]).expect_err("the parent is a regular file");
    assert_eq!(err.code, ErrorCode::Internal);
    // `create_dir_all` reports the obstruction by name (EEXIST here), so the
    // refusal names the directory it could not create.
    assert!(
        err.message.contains("failed to create")
            && err.message.contains(&blocker.display().to_string()),
        "{err}"
    );
    // The same obstruction through the transactional API, which takes the hold
    // first: the lock file cannot be created either, so neither mutation gets
    // far enough to write anything.
    for err in [
        add_token(&store, "a", "agent", &["demo".to_owned()]).expect_err("add"),
        revoke_token(&store, "a").expect_err("revoke"),
    ] {
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(err.message.contains("token store lock"), "{err}");
    }
    let _ = fs::remove_file(&blocker);
}

#[test]
fn a_store_this_process_cannot_read_is_a_corrupt_store() {
    // The owner-only check is about group and other, so a 0200 file still
    // passes it — nobody but the owner can see it. The read is what then
    // fails, and it fails closed as a corrupt store rather than being read as
    // an empty one. The refusal names the path and writes nothing.
    //
    // Only a process the mode bits restrict can observe that refusal: root, and
    // any process holding `CAP_DAC_OVERRIDE`, reads a 0200 file. The process's
    // own read of the locked store decides which of the two cases this is, so a
    // root run reports the skip rather than failing on a refusal it cannot see.
    let path = scratch("unreadable");
    save_auth_tokens(&path, &[sample_record("a")]).expect("save");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o200)).expect("lock it down");
        if fs::read(&path).is_err() {
            let err = load_auth_tokens(&path).expect_err("unreadable");
            assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{err}");
            assert!(err.message.contains("failed to read token store"), "{err}");
            assert!(err.message.contains(&path.display().to_string()), "{err}");
        } else {
            eprintln!("skipping the read refusal: this process reads a 0200 file");
        }
        // The bytes are untouched by a refusal.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("restore");
        assert_eq!(load_auth_tokens(&path).expect("readable again").len(), 1);
    }
    let _ = fs::remove_file(&path);
}
