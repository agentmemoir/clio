// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio auth token` commands: the add → list → revoke
//! flow, the refusal rules, the one-time secret print, and the help output.
//!
//! ## Owns
//! - Behavior tests for the `auth` CLI surface
//!
//! ## Does not own
//! - The command implementation under test (`auth_cli`)
//! - Store-level validation tests (they live in `clio-config`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

/// A unique scratch directory per test; every test owns its own store file, so
/// the tests run in any order and in parallel.
pub(super) fn scratch(name: &str) -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-auth-cli-{name}-{n}"))
}

/// Run one verb with `CLIO_AUTH_TOKEN_FILE` pointed at the scratch store.
pub(super) fn run_auth(store: &std::path::Path, args: &[&str]) -> (ExitCode, String, String) {
    let store_path = store.display().to_string();
    let env = move |key: &str| -> Option<String> {
        (key == "CLIO_AUTH_TOKEN_FILE").then(|| store_path.clone())
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty: false,
            no_color: true,
            env: &env,
        };
        run(&mut io, tokens(args))
    };
    (
        code,
        String::from_utf8(out).expect("utf8"),
        String::from_utf8(err).expect("utf8"),
    )
}

pub(super) fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

pub(super) fn add_args(label: &'static str, banks: &'static [&'static str]) -> Vec<&'static str> {
    let mut args = vec!["token", "add", "--label", label, "--actor", "agent"];
    for bank in banks {
        args.push("--bank");
        args.push(bank);
    }
    args
}

/// The store's records parsed straight from the file bytes.
pub(super) fn records_on_disk(store: &std::path::Path) -> Vec<Value> {
    let text = fs::read_to_string(store).expect("store file exists");
    serde_json::from_str(&text).expect("store parses as an array of records")
}

#[test]
fn add_prints_the_secret_once_and_stores_only_the_digest() {
    let store = scratch("add");
    // Text mode: the secret is its own stdout line, printed exactly once.
    let mut args = add_args("ci", &["demo"]);
    args.push("--output");
    args.push("text");
    let (code, out, err) = run_auth(&store, &args);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(err, "");
    let secret = out
        .lines()
        .find(|line| line.len() == 43)
        .expect("a 43-char secret line")
        .to_owned();
    assert_eq!(
        out.matches(&secret).count(),
        1,
        "the secret prints exactly once: {out}"
    );
    assert!(out.contains("cannot be recovered"), "{out}");
    assert!(out.contains(&store.display().to_string()), "{out}");

    // JSON mode carries the same one-time secret in the payload.
    let json_store = scratch("add-json");
    let mut json_args = add_args("ci", &["demo"]);
    json_args.extend(["--output", "json"]);
    let (code, out, err) = run_auth(&json_store, &json_args);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    let secret = payload["token"].as_str().expect("token").to_owned();
    assert_eq!(secret.len(), 43, "{payload}");
    assert_eq!(out.matches(&secret).count(), 1, "printed exactly once");
    assert_eq!(payload["store"], json!(json_store.display().to_string()));
    assert!(
        payload["note"]
            .as_str()
            .expect("note")
            .contains("cannot be recovered")
    );
    let _ = fs::remove_file(&json_store);

    let records = records_on_disk(&store);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record["label"], "ci");
    assert_eq!(record["actor"], "agent");
    assert_eq!(record["banks"], serde_json::json!(["demo"]));
    // The digest is the only secret-derived value on disk, and it matches the
    // printed secret.
    let digest = record["secret_sha256"].as_str().expect("digest");
    assert_eq!(digest.len(), 64);
    assert!(!fs::read_to_string(&store).expect("bytes").contains(&secret));

    // A second add with the same label is a domain error and the file is unchanged.
    let bytes_before = fs::read_to_string(&store).expect("bytes");
    let (code, _, err) = run_auth(&store, &add_args("ci", &["demo"]));
    assert_eq!(
        code,
        ExitCode::from(1),
        "duplicate label is an operational error: {err}"
    );
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(fs::read_to_string(&store).expect("bytes"), bytes_before);
    let _ = fs::remove_file(&store);
}

#[test]
fn bad_add_input_uses_domain_or_usage_exit_without_touching_the_store() {
    let store = scratch("bad-add");
    let cases: Vec<(Vec<&str>, u8)> = vec![
        (add_args("has space", &["demo"]), 1),
        (add_args("", &["demo"]), 2),
        (
            vec![
                "token", "add", "--label", "ok", "--actor", "robot", "--bank", "demo",
            ],
            1,
        ),
        (vec!["token", "add", "--label", "ok", "--actor", "agent"], 2),
    ];
    for (args, exit) in cases {
        let (code, _, err) = run_auth(&store, &args);
        assert_eq!(code, ExitCode::from(exit), "{args:?}: {err}");
        assert!(
            fs::read_to_string(&store).is_err(),
            "no store file is created"
        );
        assert!(!err.is_empty(), "{args:?}");
    }
    let (_, _, err) = run_auth(
        &store,
        &[
            "token", "add", "--label", "ok", "--actor", "robot", "--bank", "demo",
        ],
    );
    assert!(err.contains("agent|harness|user|system"), "{err}");
    let (_, _, err) = run_auth(
        &store,
        &["token", "add", "--label", "ok", "--actor", "agent"],
    );
    assert!(err.contains("requires --bank"), "{err}");
}

#[test]
fn list_shows_records_and_never_a_secret_or_digest() {
    let store = scratch("list");
    let (code, _, _) = run_auth(&store, &add_args("ci", &["demo"]));
    assert_eq!(code, ExitCode::SUCCESS);
    let (code, _, _) = run_auth(&store, &add_args("ops", &["demo", "other"]));
    assert_eq!(code, ExitCode::SUCCESS);

    let (code, out, err) = run_auth(&store, &["token", "list"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    let rows = payload["records"].as_array().expect("records");
    assert_eq!(rows.len(), 2);
    let labels: Vec<&str> = rows.iter().filter_map(|r| r["label"].as_str()).collect();
    assert!(
        labels.contains(&"ci") && labels.contains(&"ops"),
        "{rows:?}"
    );
    for row in rows {
        assert!(row["actor"].is_string(), "{row}");
        assert!(row["created_at"].as_str().is_some(), "{row}");
        assert!(row["banks"].is_array(), "{row}");
        // Negative assertions: neither a secret nor a digest may print.
        assert!(row.get("secret_sha256").is_none(), "{row}");
        assert!(row.get("token").is_none(), "{row}");
        let rendered = row.to_string();
        assert!(!rendered.contains("secret_sha256"), "{rendered}");
    }
    let text = out.as_str();
    assert!(!text.contains("secret_sha256"), "{text}");
    assert!(text.contains("created_at"), "{text}");
    // Text mode names the label, actor, banks, and creation stamp.
    assert!(text.contains("ci") && text.contains("ops"), "{text}");

    // `list` before any `add` shows none and creates nothing.
    let empty = scratch("list-empty");
    let (code, out, _) = run_auth(&empty, &["token", "list"]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("\"records\":[]"), "{out}");
    assert!(!empty.exists(), "list must not create the store file");
    let _ = fs::remove_file(&store);
}

#[test]
fn revoke_removes_a_record_and_states_immediate_effect() {
    let store = scratch("revoke");
    let (code, _, _) = run_auth(&store, &add_args("ci", &["demo"]));
    assert_eq!(code, ExitCode::SUCCESS);
    let before = records_on_disk(&store);

    let (code, out, err) = run_auth(&store, &["token", "revoke", "--label", "ci"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        out.contains("without a restart")
            && out.contains("the sync server applies it at its next request"),
        "{out}"
    );
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["label"], "ci");
    assert_eq!(payload["effective"], json!("immediately"));
    assert_eq!(
        payload["note"],
        json!(
            "the running MCP HTTP server applies the change without a restart, while the sync server applies it at its next request"
        )
    );
    let records = records_on_disk(&store);
    assert!(records.is_empty(), "{records:?}");
    // A reload sees the record gone.
    let (code, out, _) = run_auth(&store, &["token", "list"]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("\"records\":[]"), "{out}");

    // Unknown label: operational not-found, file unchanged.
    let bytes_before = fs::read_to_string(&store).expect("bytes");
    let (code, _, err) = run_auth(&store, &["token", "revoke", "--label", "ghost"]);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("no token labelled"), "{err}");
    assert_eq!(fs::read_to_string(&store).expect("bytes"), bytes_before);
    assert_eq!(records_on_disk(&store).len(), before.len() - 1);

    let (code, _, err) = run_auth(
        &store,
        &["token", "revoke", "--label", "ghost", "--output", "json"],
    );
    assert_eq!(code, ExitCode::from(1), "{err}");
    let error: Value = serde_json::from_str(err.trim()).expect("error envelope");
    assert_eq!(error["ok"], serde_json::json!(false), "{error}");
    assert_eq!(error["code"], serde_json::json!("not_found"), "{error}");
    assert_eq!(fs::read_to_string(&store).expect("bytes"), bytes_before);
    let _ = fs::remove_file(&store);
}

#[test]
fn a_broken_store_file_fails_every_verb_as_operational() {
    let store = scratch("refused");
    fs::write(&store, "not json").expect("write malformed");
    for args in [
        &["token", "list"][..],
        &[
            "token", "add", "--label", "a", "--actor", "agent", "--bank", "b",
        ][..],
    ] {
        let (code, _, err) = run_auth(&store, args);
        assert_eq!(code, ExitCode::from(1), "{args:?}: {err}");
        assert!(
            err.contains("malformed") || err.contains("token store"),
            "{err}"
        );
    }
    // An empty file is also refused, not parsed as an empty store.
    fs::write(&store, "").expect("write empty");
    let (code, _, err) = run_auth(&store, &["token", "list"]);
    assert_eq!(code, ExitCode::from(1), "{err}");
    // A world-readable file is refused before it is parsed.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(&store, "[]").expect("write array");
        fs::set_permissions(&store, fs::Permissions::from_mode(0o644)).expect("loosen");
        let (code, _, err) = run_auth(&store, &["token", "list"]);
        assert_eq!(code, ExitCode::from(1), "{err}");
        assert!(err.contains("owner-only"), "{err}");
    }
    let _ = fs::remove_file(&store);
}

#[test]
fn repeated_bank_flags_collapse_and_keep_order() {
    let store = scratch("banks");
    let (code, _, err) = run_auth(&store, &add_args("multi", &["demo", "demo", "other"]));
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let records = records_on_disk(&store);
    assert_eq!(
        records[0]["banks"],
        serde_json::json!(["demo", "other"]),
        "{records:?}"
    );
    let _ = fs::remove_file(&store);
}
