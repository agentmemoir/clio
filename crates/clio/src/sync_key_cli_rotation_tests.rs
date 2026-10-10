// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests `clio sync keygen` under rotation: a second generation
//! supersedes the stored key instead of refusing, prints the new key once with
//! both notes, and reports the retained key count.
//!
//! ## Owns
//! - Every test of `keygen` on a store that already holds a key
//!
//! ## Does not own
//! - A first `keygen` on an empty store (see `sync_key_cli_tests`)
//! - The temp directory and injected environment all suites use (see
//!   `sync_key_cli_test_support`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every test injects the
//! environment through `CliIo` and points the store at a self-cleaning temp
//! directory, so no test reaches the process environment or the OS key store.

use super::*;
use crate::sync_key_cli_test_support::{TempDir, stored_key, tokens};
use clio_config::SYNC_KEY_FILE_ENV;
use serde_json::Value;
use std::fs;

/// The environment that points the file store at `dir`.
fn file_env(dir: &TempDir) -> impl Fn(&str) -> Option<String> {
    let path = dir.key().display().to_string();
    move |key: &str| (key == SYNC_KEY_FILE_ENV).then(|| path.clone())
}

/// Run one key verb against injected streams and environment.
fn run_key(
    args: &[&str],
    env: &dyn Fn(&str) -> Option<String>,
    is_tty: bool,
) -> (ExitCode, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty,
            no_color: true,
            env,
        };
        run(&mut io, tokens(args))
    };
    (
        code,
        String::from_utf8(out).expect("utf8"),
        String::from_utf8(err).expect("utf8"),
    )
}

/// A second `keygen` rotates instead of refusing.
///
/// The new key prints exactly once with the recovery note, the retained key
/// adds the rotation note on both output modes, and the current key still
/// reads back through `key export`. No output mode carries either key byte
/// beyond the one deliberate print of the newest key.
#[test]
fn a_second_keygen_rotates_prints_the_new_key_once_and_keeps_older_content_readable() {
    let dir = TempDir::new("keygen-rotate");
    let env = file_env(&dir);
    let first = stored_key(&env).base64();

    let (code, out, err) = run_key(&["keygen", "--store", "file"], &env, true);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let second = out.lines().next().expect("the new key is the first line");
    assert!(!second.is_empty(), "{out}");
    assert_ne!(second, first, "a rotation writes a new key: {out}");
    assert_eq!(
        out.matches(second).count(),
        1,
        "the new key is printed exactly once: {out}"
    );
    assert!(
        out.contains("cannot be recovered except with `clio sync key export`"),
        "the recovery note stays: {out}"
    );
    assert!(out.contains("older content stays readable"), "{out}");
    assert!(out.contains("peers need this new key"), "{out}");
    assert!(
        !err.contains(second),
        "stderr must not carry the key: {err}"
    );
    assert_eq!(
        fs::read(dir.key())
            .expect("the rotated key file is written")
            .len(),
        32
    );

    let (code, exported, err) = run_key(&["key", "export"], &env, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(exported.trim(), second, "the current key reads back");
}

/// The JSON payload reports the retained key count and the rotation note.
///
/// The count comes from the store's report, so a rotation reports two held
/// keys. A first `keygen` on an empty store reports one and carries no
/// rotation note, because nothing was kept.
#[test]
fn keygen_json_reports_the_retained_key_count_and_the_rotation_note() {
    let dir = TempDir::new("keygen-json-rotate");
    let env = file_env(&dir);
    stored_key(&env);

    let (code, out, err) = run_key(&["keygen", "--output", "json"], &env, true);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["count"], Value::from(2), "{payload}");
    assert_eq!(payload["store"], Value::from("file"), "{payload}");
    assert!(
        payload["key"].as_str().is_some_and(|key| !key.is_empty()),
        "{payload}"
    );
    assert!(
        payload["rotation_note"]
            .as_str()
            .is_some_and(|note| note.contains("older content stays readable")
                && note.contains("peers need this new key")),
        "{payload}"
    );

    let fresh = TempDir::new("keygen-json-fresh");
    let (code, out, err) = run_key(&["keygen", "--output", "json"], &file_env(&fresh), true);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["count"], Value::from(1), "{payload}");
    assert!(
        payload.get("rotation_note").is_none(),
        "nothing was kept, so there is no rotation note: {payload}"
    );
}
