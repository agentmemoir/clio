// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP sync tools' encryption-key handling: the
//! `sync_status` `sync_key` override (one string or a newest-first array,
//! invalid-entry rejection, and the custody summary fields), and the
//! transfer path's key read from the explicit key file or the recorded store.
//!
//! ## Owns
//! - Test coverage for `cipher_keys`, `apply_custody_fallback`, and
//!   `custody_cipher` over the injected environment seam
//! - The pins for the accepted custody divergence: transfer reads the explicit
//!   key file first, the status custody read follows the recorded marker
//!
//! ## Does not own
//! - The tool behavior itself (`sync_tools.rs` or `clio-sync`)
//! - The dispatch and schema tests (see `sync_tools_tests`)
//!
//! ## Boundary
//!
//! Tests here break when sync key handling changes; they never change it.
//! Every custody lookup runs through an injected getter, so no test reads the
//! machine's own key store.
//!
//! Keep this module focused on MCP sync key handling tests.

use super::sync_tools_test_support_tests::{TempDir, assert_no_key_material, state};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::path::Path;

/// The `sync_key` argument takes one JSON string or a newest-first array.
///
/// A single string keeps the old calls working; an array carries a rotation
/// and the summary counts them. Absent means encryption off. Store and
/// fingerprint are pinned through the seam below, not here: a real machine's
/// key store would decide them.
#[test]
fn sync_status_takes_one_key_string_or_a_newest_first_list() {
    let st = state();
    let one = dispatch(&st, "sync_status", &json!({ "sync_key": "k1" }));
    assert_eq!(one["encryption_summary"]["key_count"], 1, "{one}");
    let list = dispatch(&st, "sync_status", &json!({ "sync_key": ["k2", "k1"] }));
    assert_eq!(list["encryption_summary"]["key_count"], 2, "{list}");
    let none = dispatch(&st, "sync_status", &json!({}));
    assert_eq!(none["encryption"], Value::Bool(false), "{none}");
    assert!(none.get("encryption_summary").is_none(), "{none}");
}

/// A non-string entry, or a non-string argument, is an invalid argument.
#[test]
fn sync_status_rejects_a_non_string_key_entry() {
    let st = state();
    let entry = dispatch(&st, "sync_status", &json!({ "sync_key": ["k1", 7] }));
    assert_eq!(entry["code"], "invalid_argument", "{entry}");
    let scalar = dispatch(&st, "sync_status", &json!({ "sync_key": 7 }));
    assert_eq!(scalar["code"], "invalid_argument", "{scalar}");
}

/// The custody summary names the store only when the machine's own key store
/// answers with a key. Supplied keys have no custody record, so the store and
/// fingerprint stay null with no stored key even though the client encrypts.
/// The injected getter is the seam: no test reads the real environment.
#[test]
fn sync_status_summary_names_the_store_only_when_custody_holds_a_key() {
    let st = state();
    let args = json!({ "sync_key": ["k1"] });
    let none = crate::sync_tools::sync_status_with(&st, &args, &|_| None).expect("status");
    let none_summary = &none["encryption_summary"];
    assert_eq!(none_summary["key_count"], 1);
    assert!(none_summary["store"].is_null());
    assert!(none_summary["current_fingerprint"].is_null());

    let dir = TempDir::new("mcp-status-custody");
    let path = dir.key().display().to_string();
    let get = |key: &str| (key == clio_config::SYNC_KEY_FILE_ENV).then(|| path.clone());
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("generate");
    let custody = crate::sync_tools::sync_status_with(&st, &args, &get).expect("status");
    let custody_summary = &custody["encryption_summary"];
    assert_eq!(custody_summary["store"], json!("file"));
    let fingerprint = custody_summary["current_fingerprint"]
        .as_str()
        .expect("a fingerprint from custody");
    assert_eq!(fingerprint.len(), 16);
    assert!(!custody.to_string().contains("key_b64"));
}

/// Status is the first place an operator looks, so with no keys supplied the
/// node's own custody answers: the report shows encryption on, the count of
/// every held key, and the store and fingerprint of the newest one.
#[test]
fn sync_status_reports_the_node_s_own_keys_when_none_are_supplied() {
    let st = state();
    let dir = TempDir::new("mcp-status-own-keys");
    let path = dir.key().display().to_string();
    let get = |key: &str| (key == clio_config::SYNC_KEY_FILE_ENV).then(|| path.clone());
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("generate");
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("rotate");

    let report = crate::sync_tools::sync_status_with(&st, &json!({}), &get).expect("status");

    assert_eq!(report["encryption"], Value::Bool(true));
    let summary = &report["encryption_summary"];
    assert_eq!(summary["key_count"], 2);
    assert_eq!(summary["store"], json!("file"));
    let fingerprint = summary["current_fingerprint"]
        .as_str()
        .expect("fingerprint");
    assert_eq!(fingerprint.len(), 16);
    assert!(!report.to_string().contains("key_b64"));
}

/// Transfer and status answer from one source: for the same arguments and the
/// same custody view, the push response's `encryption` field and the status
/// report's `encryption` agree in both directions. The getter is injected, so
/// no test reads the machine's own key store.
#[test]
fn sync_transfer_and_sync_status_agree_on_encryption_for_the_same_arguments() {
    let st = state();
    let serve = dispatch(&st, "sync_serve", &json!({ "dev": true, "ttl_secs": 20 }));
    assert_eq!(serve["ok"], Value::Bool(true), "{serve}");
    let addr = serve["addr"].as_str().expect("addr").to_owned();

    // No keys in custody: both surfaces report encryption off. The getter
    // answers `None`, so no explicit file is named and no recorded store
    // resolves either; an explicit path that held no key would be an error.
    let none = |_key: &str| None;
    let pushed =
        crate::sync_tools::sync_transfer_with(&st, &json!({ "remote": addr }), "push", &none)
            .expect("push");
    let status = crate::sync_tools::sync_status_with(&st, &json!({}), &none).expect("status");
    assert_eq!(pushed["encryption"], Value::Bool(false));
    assert_eq!(pushed["encryption"], status["encryption"]);

    // The node holds keys and the caller supplies none: both surfaces report
    // encryption on, from the same custody set. Generating twice leaves a
    // current key plus an archived one, so the count proves both were read.
    let dir = TempDir::new("mcp-transfer-custody");
    let path = dir.key().display().to_string();
    let get = |key: &str| (key == clio_config::SYNC_KEY_FILE_ENV).then(|| path.clone());
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("generate");
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("rotate");
    let pushed =
        crate::sync_tools::sync_transfer_with(&st, &json!({ "remote": addr }), "push", &get)
            .expect("push");
    let status = crate::sync_tools::sync_status_with(&st, &json!({}), &get).expect("status");
    assert_eq!(pushed["encryption"], Value::Bool(true));
    assert_eq!(pushed["encryption"], status["encryption"]);
    assert_eq!(status["encryption_summary"]["key_count"], 2);
}

/// The transfer path's custody read is ordered: the newest key first, then
/// every archived key. Sealing uses the first entry, so the order decides
/// which key new content is sealed under. That an archived key still opens
/// content sealed under it is proven where the keys are used (see `clio-sync`'s
/// client rotation tests); this test pins the read and its order.
#[test]
fn sync_transfer_without_a_key_reads_newest_and_archived_keys_from_custody() {
    let st = state();
    let dir = TempDir::new("mcp-transfer-rotation");
    let path = dir.key().display().to_string();
    let get = |key: &str| (key == clio_config::SYNC_KEY_FILE_ENV).then(|| path.clone());
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("generate");
    let archived =
        clio_config::sync_key::read_key_file_base64(Path::new(&path)).expect("the first key");
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("rotate");
    let newest =
        clio_config::sync_key::read_key_file_base64(Path::new(&path)).expect("the second key");

    // The exact ordered list: the file's key seals, the archived key follows.
    let keys = super::custody_secrets(&get).expect("custody keys");
    assert_eq!(keys, vec![newest, archived], "newest first, archived after");

    // The MCP-facing helper carries the same two keys.
    let cipher = super::custody_cipher(&get).expect("custody cipher");
    assert_eq!(cipher.len(), 2, "newest plus archived");

    let serve = dispatch(&st, "sync_serve", &json!({ "dev": true, "ttl_secs": 20 }));
    assert_eq!(serve["ok"], Value::Bool(true), "{serve}");
    let addr = serve["addr"].as_str().expect("addr").to_owned();
    let pushed =
        crate::sync_tools::sync_transfer_with(&st, &json!({ "remote": addr }), "push", &get)
            .expect("push");
    assert_eq!(pushed["encryption"], Value::Bool(true));
}

/// The explicit `CLIO_SYNC_KEY_FILE` outranks a recorded store marker on the
/// transfer path.
///
/// The marker is rewritten to `keystore` after a file generation, the state a
/// machine that recorded the other store is in; the transfer read must still
/// answer from the file the environment names. The injected getter is the seam:
/// no test reads the real environment or the machine's own key store.
#[test]
fn the_explicit_key_file_wins_over_the_recorded_store() {
    let dir = TempDir::new("mcp-file-over-store");
    let path = dir.key();
    let get =
        |key: &str| (key == clio_config::SYNC_KEY_FILE_ENV).then(|| path.display().to_string());
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("generate");
    let stored = clio_config::sync_key::read_key_file_base64(&path).expect("the file key");

    let marker = path
        .parent()
        .expect("the key file has a directory")
        .join("sync-key-source");
    std::fs::write(&marker, "keystore\n").expect("record the keystore");

    let keys = super::custody_secrets(&get).expect("the file key resolves");

    assert_eq!(
        keys,
        vec![stored],
        "the explicit file answers even though the marker names another store"
    );
}

/// A custody store that cannot be read is an error on the transfer path, the
/// same rule the status path follows: encryption never silently turns off
/// between two surfaces that both report it.
#[test]
fn sync_transfer_errors_when_the_custody_store_cannot_be_read() {
    let st = state();
    let dir = TempDir::new("mcp-transfer-damaged");
    let path = dir.key();
    std::fs::write(&path, b"short").expect("write a damaged key file");
    let get =
        |key: &str| (key == clio_config::SYNC_KEY_FILE_ENV).then(|| path.display().to_string());

    let result = crate::sync_tools::sync_transfer_with(
        &st,
        &json!({ "remote": "127.0.0.1:1" }),
        "push",
        &get,
    );

    assert!(result.is_err(), "a damaged custody store is an error");
}

/// The accepted custody divergence, pinned with a store the tests can reach.
///
/// `CLIO_SYNC_KEY_FILE` names a file that is missing, and the recorded marker
/// names the file store. The transfer read takes the named file first and
/// refuses when it is missing, where the recorded store alone would have
/// answered "no key". The status custody read follows the marker instead, so
/// it still answers: the archived key the rotation left behind keeps
/// encryption on. Neither surface silently turns encryption off.
#[test]
fn transfer_refuses_a_missing_named_file_while_status_answers_from_the_archive() {
    let st = state();
    let dir = TempDir::new("mcp-divergence-missing-file");
    let path = dir.key().display().to_string();
    let get = |key: &str| (key == clio_config::SYNC_KEY_FILE_ENV).then(|| path.clone());
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("generate");
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("rotate");
    std::fs::remove_file(&path).expect("remove the current key file");

    let transfer = crate::sync_tools::sync_transfer_with(
        &st,
        &json!({ "remote": "127.0.0.1:1" }),
        "push",
        &get,
    );
    assert!(
        transfer.is_err(),
        "a named key file that is missing is an error on the transfer path"
    );

    let status = crate::sync_tools::sync_status_with(&st, &json!({}), &get).expect("status");
    assert_eq!(status["encryption"], Value::Bool(true));
    assert_eq!(status["encryption_summary"]["key_count"], 1);
}

/// The transfer read and the status custody read follow different masters when
/// a keystore marker is recorded and `CLIO_SYNC_KEY_FILE` names a file.
///
/// The marker is rewritten to `keystore` after a file generation, the state a
/// machine that recorded the other store is in. The transfer read is
/// file-first: the file's key seals. The status read is marker-authoritative:
/// it answers from the recorded keystore and never consults the named file.
/// The keystore read itself is not driven here, because a test may not reach
/// the machine's own OS secret store; what is pinned is the transfer half and
/// the source decision the status read follows. The missing-file test above
/// pins the same divergence end to end with a store the tests can reach.
#[test]
fn transfer_seals_with_the_file_key_over_a_keystore_marker() {
    let dir = TempDir::new("mcp-file-over-keystore");
    let path = dir.key();
    let get =
        |key: &str| (key == clio_config::SYNC_KEY_FILE_ENV).then(|| path.display().to_string());
    clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File)).expect("generate");
    let file_key = clio_config::sync_key::read_key_file_base64(&path).expect("the file key");
    let marker = path
        .parent()
        .expect("the key file has a directory")
        .join("sync-key-source");
    std::fs::write(&marker, "keystore\n").expect("record the keystore");

    let keys = super::custody_secrets(&get).expect("the file key resolves");
    assert_eq!(
        keys,
        vec![file_key],
        "the transfer seals with the file's key even though the marker names the keystore"
    );
    assert_eq!(
        clio_config::sync_key::recorded_source(&get)
            .expect("the marker is readable")
            .expect("a store is recorded"),
        clio_config::KeySource::Keystore,
        "the source the status custody read follows is the recorded marker, not the named file"
    );
}

#[test]
fn sync_key_status_reports_store_fingerprint_time_and_count_without_a_key() {
    let result = crate::sync_tools::sync_key_status_with(&|_| None).expect("report");

    assert_eq!(result["store"], "file");
    assert_eq!(result["count"], 0);
    assert!(result["fingerprint"].is_null());
    assert!(result["created_at"].is_null());
    assert_eq!(
        result["keys"].as_array().map(Vec::len),
        Some(0),
        "an empty report lists no keys"
    );
    assert!(
        result["timestamp"]
            .as_str()
            .is_some_and(|stamp| stamp.ends_with('Z'))
    );
    // The empty report carries no key, so the field-name scan is all there is to
    // assert here; the stored-key case below is what makes the rule real.
    let rendered = result.to_string();
    assert!(!rendered.contains("\"key\""));
    for name in ["key_b64", "secret", "key_bytes", "sync_key"] {
        assert!(!rendered.contains(name), "a leaked field");
    }
}

/// The never-leak rule on a node that actually holds a key.
///
/// The empty-report case above cannot tell a handler that leaks from one that
/// does not: with nothing stored there is nothing to leak, so any field name
/// would satisfy it. This drives the handler over a real key file, where a
/// regression that printed a field, a prefix of the base64, or the raw bytes
/// would show up.
#[test]
fn sync_key_status_over_a_stored_key_carries_no_key_byte_and_no_prefix() {
    let dir = TempDir::new("mcp-key-status");
    let path = dir.key().display().to_string();
    let get = |key: &str| (key == clio_config::SYNC_KEY_FILE_ENV).then(|| path.clone());

    // The production generator, so the assertion below is about the handler's
    // output rather than about a fixture shaped like a key.
    let generated = clio_config::sync_key::generate(&get, Some(clio_config::KeySource::File))
        .expect("generate into the temp directory");

    let result = crate::sync_tools::sync_key_status_with(&get).expect("report");

    assert_eq!(result["count"], 1, "the premise: a key is stored");
    let fingerprint = result["fingerprint"]
        .as_str()
        .expect("a fingerprint for the stored key");
    assert_eq!(fingerprint.len(), 16);
    assert!(
        fingerprint
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "lowercase hex only: {fingerprint}"
    );
    let keys = result["keys"].as_array().expect("the keys list");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["fingerprint"], fingerprint);
    assert_eq!(keys[0]["current"], json!(true));
    assert_eq!(keys[0]["store"], json!("file"));
    assert_no_key_material(&result, fingerprint, &generated.base64());
}
