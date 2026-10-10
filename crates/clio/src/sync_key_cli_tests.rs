// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio sync` key verbs: keygen output and refusals,
//! export and status masking, and the help and entrypoint paths.
//!
//! ## Owns
//! - Every test of `keygen` on an empty store, and of `key export` and
//!   `key status`
//!
//! ## Does not own
//! - The key lifecycle itself (see `clio-config`'s `sync_key_tests`)
//! - The temp directory and injected environment both suites use (see
//!   `sync_key_cli_test_support`)
//! - The `sync push`/`pull` flag decoding (see `sync_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every test injects the
//! environment through `CliIo` and points the store at a self-cleaning temp
//! directory, so no test reaches the process environment or the OS key store.

use super::*;
use crate::sync_key_cli_test_support::{TempDir, env_of, stored_key, tokens};
use clio_config::SYNC_KEY_FILE_ENV;
use serde_json::Value;
use std::fs;

/// The environment that points the file store at `dir`.
fn file_env(dir: &TempDir) -> impl Fn(&str) -> Option<String> {
    env_of(vec![(SYNC_KEY_FILE_ENV, dir.key().display().to_string())])
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

#[test]
fn keygen_json_reports_the_key_fingerprint_store_and_count() {
    let dir = TempDir::new("keygen-json");
    let env = file_env(&dir);

    // No `--store`: the recommended file store is the default.
    let (code, out, err) = run_key(&["keygen", "--output", "json"], &env, true);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["store"], json!("file"), "{payload}");
    assert_eq!(payload["count"], json!(1), "{payload}");
    assert!(
        payload["key"].as_str().is_some_and(|key| !key.is_empty()),
        "{payload}"
    );
    assert!(
        payload["fingerprint"]
            .as_str()
            .is_some_and(|fingerprint| fingerprint.len() == 16),
        "{payload}"
    );
    assert!(
        payload["created_at"].is_string() || payload["created_at"].is_null(),
        "{payload}"
    );
    assert!(
        payload["note"]
            .as_str()
            .is_some_and(|note| note.contains("clio sync key export")),
        "{payload}"
    );
}

/// A report that cannot be read leaves the count unknown, not guessed.
///
/// The count comes from the store's report, so a damaged archive makes that
/// report fail after the key itself was written. A guessed 1 would tell a node
/// holding several keys it holds one, so the field stays present and null.
///
/// The archive is created damaged by hand, under the name the archive lives
/// beside the key file under. Generation on an empty store never reads it, so
/// the key is written normally and only the report refuses.
#[test]
fn keygen_json_reports_an_unknown_count_when_the_report_cannot_be_read() {
    let dir = TempDir::new("keygen-json-unreadable");
    let env = file_env(&dir);
    fs::write(dir.path().join("clio-sync-keys-archive.json"), "not json\n")
        .expect("the damaged archive is written");

    let (code, out, err) = run_key(&["keygen", "--output", "json"], &env, true);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert!(
        payload["count"].is_null(),
        "an unreadable report must not guess a count: {payload}"
    );
    assert!(
        payload["key"].as_str().is_some_and(|key| !key.is_empty()),
        "the key itself was written: {payload}"
    );
}

#[test]
fn keygen_rejects_an_unknown_store_with_the_accepted_names() {
    let dir = TempDir::new("keygen-bad-store");
    let env = file_env(&dir);

    let (code, out, err) = run_key(&["keygen", "--store", "smartcard"], &env, true);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("file|keystore"), "{err}");
    assert!(out.is_empty(), "{out}");
}

/// A store choice that cannot be recorded leaves no key behind.
///
/// The marker path is a dangling symlink: reading it is "no record yet" while
/// creating it is refused. Because the record is written before the key bytes, a
/// refusal here means nothing was stored, so there is no key for an operator to
/// lose and no second `keygen` to be tempted into.
#[cfg(unix)]
#[test]
fn a_record_failure_writes_no_key() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new("keygen-record");
    symlink(
        dir.path().join("missing-target"),
        dir.path().join("sync-key-source"),
    )
    .expect("the marker path is a dangling symlink");
    let env = file_env(&dir);

    let (code, out, err) = run_key(&["keygen", "--store", "file"], &env, true);

    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(err.contains("key-store record"), "{err}");
    assert!(
        out.is_empty(),
        "no key is printed when none was stored: {out}"
    );
    assert!(!dir.key().exists(), "no key file was written");
}

/// A refused `keygen` after the record is deleted leaves the stored key readable.
///
/// The refusal advises removing the record to choose a different store, and the
/// operator who does exactly that must still be able to read the key afterwards.
/// A refusal that rewrote the record would point every later reader at a store
/// the refused run never wrote to, leaving the key on disk with no verb to read it.
#[test]
fn a_refused_keygen_after_the_record_is_removed_leaves_the_key_readable() {
    let dir = TempDir::new("keygen-refused");
    let env = file_env(&dir);
    let key = stored_key(&env).base64();
    fs::remove_file(dir.path().join("sync-key-source")).expect("the record is gone");

    let (code, out, err) = run_key(&["keygen", "--store", "keystore"], &env, true);
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(err.contains("already exists"), "{err}");
    assert!(out.is_empty(), "no key is printed by a refusal: {out}");

    let (code, exported, err) = run_key(&["key", "export"], &env, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(exported.trim(), key, "the stored key still reads back");
}

#[test]
fn export_text_is_exactly_the_stored_key_and_a_newline() {
    let dir = TempDir::new("export-text");
    let env = file_env(&dir);
    let stored = stored_key(&env).base64();

    let (code, out, err) = run_key(&["key", "export"], &env, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(out, format!("{stored}\n"));
    assert!(
        err.is_empty(),
        "export writes only the key to stdout: {err}"
    );
}

#[test]
fn export_with_no_key_is_operational_and_prints_nothing() {
    let dir = TempDir::new("export-empty");
    let env = file_env(&dir);

    let (code, out, err) = run_key(&["key", "export"], &env, true);
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(out.is_empty(), "nothing is printed without a key: {out}");
    assert!(err.contains("no sync key is stored"), "{err}");
    assert!(err.contains("clio sync keygen"), "{err}");
}

#[test]
fn status_text_reports_the_facts_without_the_key() {
    let dir = TempDir::new("status-text");
    let env = file_env(&dir);
    let key = stored_key(&env).base64();

    let (code, out, err) = run_key(&["key", "status"], &env, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        out.starts_with("sync key status (store file, keys 1)"),
        "{out}"
    );
    assert!(out.contains("fingerprint "), "{out}");
    assert!(out.contains("created at "), "{out}");
    assert!(out.contains("(current)"), "{out}");
    assert_no_key_prefix(&out, &key, "the text status");
    assert_no_key_prefix(&err, &key, "stderr");
}

#[test]
fn status_json_reports_the_facts_and_never_a_key_field() {
    let dir = TempDir::new("status-json");
    let env = file_env(&dir);
    let key = stored_key(&env).base64();

    let (code, out, err) = run_key(&["key", "status", "--output", "json"], &env, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["store"], json!("file"), "{payload}");
    assert_eq!(payload["count"], json!(1), "{payload}");
    let keys = payload["keys"].as_array().expect("the keys list");
    assert_eq!(keys.len(), 1, "{payload}");
    assert_eq!(keys[0]["current"], json!(true), "{payload}");
    assert_eq!(keys[0]["store"], json!("file"), "{payload}");
    assert!(
        keys[0]["fingerprint"]
            .as_str()
            .is_some_and(|f| f.len() == 16),
        "{payload}"
    );
    assert!(
        payload["fingerprint"]
            .as_str()
            .is_some_and(|fingerprint| fingerprint.len() == 16),
        "{payload}"
    );
    assert!(
        payload["created_at"].is_string() || payload["created_at"].is_null(),
        "{payload}"
    );
    assert!(
        payload.get("key").is_none(),
        "status must not carry the key: {payload}"
    );
    assert_no_key_prefix(&out, &key, "the json status");
}

/// Assert that `haystack` carries neither the key nor any prefix of it.
///
/// The stated condition names prefixes, and asserting only the whole key would
/// pass for a handler that printed the first eight characters. The fingerprint is
/// excluded because it is derived from the key and is the one value the report is
/// supposed to carry.
fn assert_no_key_prefix(haystack: &str, key: &str, where_: &str) {
    assert!(
        !haystack.contains(key),
        "{where_} printed the key: {haystack}"
    );
    for len in [4_usize, 8, 12, 16, 20, 24] {
        let prefix = &key[..len];
        if haystack.contains(&format!("fingerprint {prefix}")) {
            continue;
        }
        assert!(
            !haystack.contains(prefix),
            "{where_} printed a {len}-character prefix of the key: {prefix} in {haystack}"
        );
    }
}

#[test]
fn status_with_no_key_says_so_without_failing() {
    let dir = TempDir::new("status-empty");
    let env = file_env(&dir);

    let (code, out, err) = run_key(&["key", "status"], &env, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("keys 0"), "{out}");
    assert!(out.contains("no sync key is stored"), "{out}");
    assert!(out.contains("clio sync keygen"), "{out}");
}

#[test]
fn unknown_key_subcommand_suggests_and_help_lists_the_verbs() {
    let (code, _, err) = run_key(&["key", "exprot"], &env_of(vec![]), true);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(
        err.contains("unknown `sync key` subcommand `exprot`"),
        "{err}"
    );
    assert!(
        err.contains("did you mean `clio sync key export`?"),
        "{err}"
    );

    let (code, out, err) = run_key(&["key", "--help"], &env_of(vec![]), true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    for verb in [
        "clio sync keygen",
        "clio sync key export",
        "clio sync key status",
    ] {
        assert!(out.contains(verb), "help must list {verb}: {out}");
    }
}

/// The nested help is rendered by the shared group renderer, not hand-written.
///
/// It must therefore carry the same binding marker `clio sync --help` prints, and
/// it must not file `clio sync keygen` under the `sync key` heading: that is a
/// `clio sync` subcommand, and listing it as a nested one is the confusion this
/// test pins.
#[test]
fn the_nested_key_help_marks_each_verb_with_its_binding() {
    let (code, out, err) = run_key(&["key", "--help"], &env_of(vec![]), true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    assert!(out.contains("`sync key` subcommands:"), "{out}");
    assert!(
        out.contains("clio sync key export   (no MCP tool)"),
        "the two printing verbs bind nothing and must say so: {out}"
    );
    assert!(
        out.contains("clio sync key status   -> sync_key_status"),
        "the read verb must show the tool it binds: {out}"
    );
    let keygen = out
        .lines()
        .find(|line| line.contains("clio sync keygen"))
        .expect("keygen is named");
    assert!(
        keygen.contains("`clio sync` subcommand"),
        "keygen is a `clio sync` subcommand, not a nested `sync key` one: {keygen}"
    );
    assert!(
        !keygen.contains("->"),
        "a hand-rolled line carries no binding marker: {keygen}"
    );
}

#[test]
fn each_key_verb_help_prints_its_registered_usage() {
    for (args, usage) in [
        (&["keygen", "--help"][..], "usage: clio sync keygen"),
        (
            &["key", "export", "--help"][..],
            "usage: clio sync key export",
        ),
        (
            &["key", "status", "--help"][..],
            "usage: clio sync key status",
        ),
    ] {
        let (code, out, err) = run_key(args, &env_of(vec![]), true);
        assert_eq!(code, ExitCode::SUCCESS, "{args:?}: {err}");
        assert!(out.contains(usage), "{args:?}: {out}");
    }
}

#[test]
fn the_entrypoint_and_odd_first_tokens_stay_usage_safe() {
    // `run` is the tested seam; `sync_key` is the thin process wrapper.
    assert_eq!(
        sync_key(vec!["key".to_owned(), "--help".to_owned()]),
        ExitCode::SUCCESS
    );
    let (code, _, err) = run_key(&["bogus"], &env_of(vec![]), true);
    assert_eq!(code, ExitCode::from(2), "{err}");

    let (code, _, err) = run_key(&["key"], &env_of(vec![]), true);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("requires a subcommand"), "{err}");

    let (code, _, err) = run_key(&["key", "--bogus"], &env_of(vec![]), true);
    assert_eq!(code, ExitCode::from(2), "{err}");
}

/// `--db` and `--backend` cannot change what a key verb does, so they are
/// refused rather than accepted and ignored.
#[test]
fn the_store_flags_are_refused_by_every_key_verb() {
    let dir = TempDir::new("key-store-flags");
    let env = file_env(&dir);
    for args in [
        &["keygen", "--store", "file", "--db", "/nonexistent/dir/x.db"][..],
        &["key", "export", "--backend", "nope"][..],
        &["key", "status", "--db", "/nonexistent/dir/x.db"][..],
    ] {
        let (code, out, err) = run_key(args, &env, true);
        assert_eq!(code, ExitCode::from(2), "{args:?}: {out}{err}");
        assert!(
            err.contains("reads the sync key, not a store"),
            "{args:?}: {err}"
        );
        assert!(out.is_empty(), "{args:?}: {out}");
    }
    assert!(!dir.key().exists(), "a refused verb writes nothing at all");
}

#[test]
fn a_positional_argument_is_refused_by_every_key_verb() {
    let dir = TempDir::new("key-positional");
    let env = file_env(&dir);
    for args in [
        &["keygen", "extra"][..],
        &["key", "export", "extra"][..],
        &["key", "status", "extra"][..],
    ] {
        let (code, out, err) = run_key(args, &env, true);
        assert_eq!(code, ExitCode::from(2), "{args:?}: {out}{err}");
        assert!(err.contains("unexpected extra argument"), "{args:?}: {err}");
    }
}
