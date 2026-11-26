// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests what `clio config set` leaves on disk when the value it
//! applies carries a secret of its own rather than sitting at a path with a
//! secret name: a deployment-scoped endpoint URL with userinfo credentials.
//!
//! ## Owns
//! - The deployment-scoped `config set` write of a URL whose userinfo holds a
//!   password: the file holds a sealed envelope, never the password, and the
//!   reported value is still masked
//! - The read a later process performs on that file, so the mask is proved
//!   against the bytes rather than against the runtime that wrote them
//!
//! ## Does not own
//! - The command implementation or its views (`config_cli`)
//! - The sealing rule itself and the envelope format (`clio-config`)
//! - The general config verb matrix and its normal-path tests (`config_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::tests::{ScratchDir, json_of, run, runtime_with_deployment_file};
use super::*;
use serde_json::json;

/// A URL whose userinfo carries a password.
const USERINFO_URL: &str = "https://user:DUMMY_PASSWORD@host";

/// A URL whose password itself contains an `@`, which only the last `@` ends.
const AT_IN_PASSWORD_URL: &str = "https://user@name:REALPASS@host";

/// A value that is not a secret but starts with the envelope prefix.
const LOOKALIKE: &str = "sealed:v1:notarealenvelope";

/// The stored text of `path` in the deployment file `name` under `dir`.
fn stored(dir: &ScratchDir, key: &str) -> Value {
    let text =
        std::fs::read_to_string(dir.join("deployment.json")).expect("the set wrote the file");
    let stored = serde_json::from_str::<Value>(&text).expect("the file is a flat JSON object");
    stored.get(key).cloned().expect("the key is in the file")
}

/// The envelope the deployment file holds at `key`.
fn envelope_at(dir: &ScratchDir, key: &str) -> String {
    let Some(Value::String(stored)) = Some(stored(dir, key)) else {
        panic!("the stored value at {key} is a string");
    };
    stored
}

#[test]
fn config_set_scope_deployment_seals_a_url_with_userinfo_and_reports_it_masked() {
    let (mut rt, dir) = runtime_with_deployment_file();

    let (code, out, err) = run(
        &["set", "extract.url", USERINFO_URL, "--scope", "deployment"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["value"], json!("[REDACTED]"), "{out}");
    assert!(!out.contains("DUMMY_PASSWORD"), "{out}");

    let text =
        std::fs::read_to_string(dir.join("deployment.json")).expect("the set wrote the file");
    assert!(
        !text.contains("DUMMY_PASSWORD"),
        "the password reached the file: {text}"
    );
    assert!(
        clio_config::secret_seal::is_sealed(&envelope_at(&dir, "extract.url")),
        "the stored value is an envelope: {text}"
    );

    let (_, applied, err) = run(
        &["set", "extract.url", USERINFO_URL, "--scope", "deployment"],
        &mut rt,
        true,
    );
    assert!(!applied.contains("DUMMY_PASSWORD"), "{applied}{err}");
    assert!(
        applied.contains("config set extract.url (scope deployment) = [REDACTED]"),
        "{applied}"
    );

    // A runtime booted from the file alone, so the read goes through the load
    // side: an in-memory overlay would hide a regression there.
    let mut later = reload(&dir);
    let (_, read_back, err) = run(&["get", "extract.url"], &mut later, true);
    assert!(!read_back.contains("DUMMY_PASSWORD"), "{read_back}{err}");
    assert!(
        read_back.contains("config extract.url = [REDACTED]"),
        "{read_back}"
    );
}

#[test]
fn config_set_seals_a_userinfo_password_that_contains_an_at_sign() {
    let (mut rt, dir) = runtime_with_deployment_file();

    let (code, out, err) = run(
        &[
            "set",
            "extract.url",
            AT_IN_PASSWORD_URL,
            "--scope",
            "deployment",
        ],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert!(!out.contains("REALPASS"), "{out}");
    assert_eq!(json_of(&out)["value"], json!("[REDACTED]"), "{out}");

    let text =
        std::fs::read_to_string(dir.join("deployment.json")).expect("the set wrote the file");
    assert!(
        !text.contains("REALPASS"),
        "the password reached the file: {text}"
    );
    assert!(
        clio_config::secret_seal::is_sealed(&envelope_at(&dir, "extract.url")),
        "the stored value is an envelope: {text}"
    );

    let mut later = reload(&dir);
    let (_, read_back, err) = run(&["get", "extract.url"], &mut later, true);
    assert!(!read_back.contains("REALPASS"), "{read_back}{err}");
    assert!(
        read_back.contains("config extract.url = [REDACTED]"),
        "{read_back}"
    );
}

#[test]
fn config_set_writes_a_lookalike_value_plain_and_a_later_read_keeps_the_file() {
    // The value is not a secret, so the save writes it as typed. The load must
    // then hand the rest of the file over rather than refuse it, which is what
    // would otherwise leave `config set` unable to repair the file.
    let (mut rt, dir) = runtime_with_deployment_file();
    for (path, value) in [
        ("backend", "postgres"),
        ("store.database_url", "postgres://user:pw@host/db"),
        ("credentials.api_key", "sk-live-abcdef0123456789"),
    ] {
        let (code, _, err) = run(
            &["set", path, value, "--scope", "deployment"],
            &mut rt,
            false,
        );
        assert_eq!(code, ExitCode::SUCCESS, "{path}: {err}");
    }
    let (code, _, err) = run(
        &["set", "extract.model", LOOKALIKE, "--scope", "deployment"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    assert_eq!(
        envelope_at(&dir, "extract.model"),
        LOOKALIKE,
        "a lookalike value stays the text the user typed"
    );

    let mut later = reload(&dir);
    let (code, backend, err) = run(&["get", "backend"], &mut later, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(json_of(&backend)["value"], json!("postgres"), "{backend}");

    let (code, api_key, err) = run(&["get", "credentials.api_key"], &mut later, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(json_of(&api_key)["value"], json!("[REDACTED]"), "{api_key}");

    // The same runtime can still set a value, so the file is repairable.
    let (code, _, err) = run(
        &["set", "extract.model", "gpt", "--scope", "deployment"],
        &mut later,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(stored(&dir, "extract.model"), json!("gpt"));
}

#[test]
fn a_moved_envelope_reads_back_as_the_opaque_text() {
    // An envelope copied to a path the save would never seal does not open
    // there, so the read hands back the sealed text rather than a secret the
    // display mask does not cover for that path.
    let (mut rt, dir) = runtime_with_deployment_file();
    let (code, _, err) = run(
        &[
            "set",
            "credentials.api_key",
            "sk-live-abcdef0123456789",
            "--scope",
            "deployment",
        ],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let envelope = envelope_at(&dir, "credentials.api_key");

    let text = std::fs::read_to_string(dir.join("deployment.json")).expect("read the file");
    let mut moved: Value = serde_json::from_str(&text).expect("the file is a flat JSON object");
    let Some(object) = moved.as_object_mut() else {
        panic!("the file is a JSON object: {text}");
    };
    object.insert("extract.model".into(), json!(envelope));
    std::fs::write(
        dir.join("deployment.json"),
        serde_json::to_string_pretty(&moved).expect("serialize"),
    )
    .expect("write the moved envelope");

    let mut later = reload(&dir);
    let (code, out, err) = run(&["get", "extract.model"], &mut later, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(!out.contains("sk-live-abcdef0123456789"), "{out}");
    assert_eq!(json_of(&out)["value"], json!(envelope), "{out}");
}

/// A runtime booted from the deployment file `dir` holds, on the same thread
/// and so the same test seal key.
fn reload(dir: &ScratchDir) -> Runtime {
    Runtime::with_env_and_deployment_file(&|_key: &str| None, dir.join("deployment.json"))
}
