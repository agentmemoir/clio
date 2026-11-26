// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the overwrite check in
//! `config::persist::load`.
//!
//! ## Owns
//! - The file the check opened, and the refusal to replace a path that no
//!   longer names that file
//! - The gate that opens an envelope wherever it sits and leaves plain config
//!   alone
//!
//! ## Does not own
//! - Production load logic under test
//! - The save side that uses the guard (see `save_tests`)
//! - The refusal shapes for a plaintext secret at a sealed path (see
//!   `seal_gate_tests`)
//!
//! ## Boundary
//!
//! Changes to what the overwrite check accepts should normally update tests
//! here. Changes to what a save writes should not.
//!
//! Keep this module focused on the overwrite check and the value gate.

use super::*;
use crate::secret_seal::test_key;
use serde_json::json;
use std::path::PathBuf;

/// The key the gate tests here seal under.
const KEY: [u8; crate::secret_key::KEY_LEN] = [0x7c; crate::secret_key::KEY_LEN];

/// A scratch directory that cleans itself up when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("am-load-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn config(&self) -> PathBuf {
        self.0.join("deployment.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn guard_accepts_the_file_it_checked() {
    let scratch = Scratch::new("same");
    let path = scratch.config();
    fs::write(&path, r#"{"log.level":"info"}"#).expect("seed");

    let guard = check_overwritable(&path).expect("a loadable file passes");

    guard
        .ensure_unchanged(&path)
        .expect("the path still names the checked file");
}

#[cfg(unix)]
#[test]
fn guard_refuses_a_path_swapped_after_the_check() {
    let scratch = Scratch::new("swapped");
    let path = scratch.config();
    fs::write(&path, r#"{"log.level":"info"}"#).expect("seed");
    let guard = check_overwritable(&path).expect("a loadable file passes");

    let swapped = "{ not json";
    let other = scratch.0.join("other.json");
    fs::write(&other, swapped).expect("swap");
    fs::rename(&other, &path).expect("replace the entry");

    let err = guard
        .ensure_unchanged(&path)
        .expect_err("a different file at the path is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(
        err.message.contains("changed while Clio checked it"),
        "the message names the change under check: {}",
        err.message
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        swapped,
        "the swapped file must keep its bytes"
    );
}

#[cfg(not(unix))]
#[test]
fn guard_accepts_a_replaced_file_where_identity_is_unavailable() {
    // This platform has no device and inode numbers, so the guard cannot tell
    // one regular file from another and accepts a replacement. The refusal of
    // a symbolic link and of a non-file entry still runs here.
    let scratch = Scratch::new("replaced");
    let path = scratch.config();
    fs::write(&path, r#"{"log.level":"info"}"#).expect("seed");
    let guard = check_overwritable(&path).expect("a loadable file passes");

    let other = scratch.0.join("other.json");
    fs::write(&other, r#"{"log.level":"debug"}"#).expect("replacement");
    fs::remove_file(&path).expect("remove");
    fs::rename(&other, &path).expect("replace the entry");

    guard
        .ensure_unchanged(&path)
        .expect("a replaced regular file is accepted without file identity");
}

#[test]
fn guard_refuses_a_file_that_appears_after_an_absent_check() {
    let scratch = Scratch::new("absent");
    let path = scratch.config();

    let guard = check_overwritable(&path).expect("a missing file passes");
    guard
        .ensure_unchanged(&path)
        .expect("the path is still missing");

    let appeared = r#"{"log.level":"debug"}"#;
    fs::write(&path, appeared).expect("appear");

    let err = guard
        .ensure_unchanged(&path)
        .expect_err("a file created under the check is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        appeared,
        "the file that appeared must keep its bytes"
    );
}

#[test]
fn guard_refuses_when_the_path_becomes_a_directory() {
    let scratch = Scratch::new("directory");
    let path = scratch.config();
    fs::write(&path, r#"{"log.level":"info"}"#).expect("seed");
    let guard = check_overwritable(&path).expect("a loadable file passes");

    fs::remove_file(&path).expect("remove");
    fs::create_dir(&path).expect("directory");

    let err = guard
        .ensure_unchanged(&path)
        .expect_err("a directory at the path is not the checked file");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}

#[test]
fn an_envelope_at_a_path_that_is_not_sealed_opens() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("gate-envelope");
    let path = scratch.config();
    let url = "https://user:DUMMY_PASSWORD@host";
    let envelope = crate::secret_seal::seal(url).expect("seal");
    fs::write(
        &path,
        serde_json::to_string_pretty(&json!({ "extract.url": envelope })).expect("serialize"),
    )
    .expect("write a file holding one envelope");

    let loaded = load_overlay_file(&path).expect("an envelope opens whatever its path");
    assert_eq!(loaded.get("extract.url"), Some(&json!(url)));
}

#[test]
fn plain_values_at_ordinary_paths_come_back_unchanged() {
    let scratch = Scratch::new("gate-plain");
    let path = scratch.config();
    fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "extract.url": "https://host/v1",
            "log.level": "debug",
            "history.lesson_max_tokens": 40,
        }))
        .expect("serialize"),
    )
    .expect("write a plain file");

    let loaded = load_overlay_file(&path).expect("plain config loads");
    assert_eq!(loaded.get("extract.url"), Some(&json!("https://host/v1")));
    assert_eq!(loaded.get("log.level"), Some(&json!("debug")));
    assert_eq!(loaded.get("history.lesson_max_tokens"), Some(&json!(40)));
}

#[test]
fn a_plaintext_secret_at_a_sealed_path_still_refuses_the_file() {
    // The refusal this gate has always applied must survive the value rule: a
    // readable credential in the file is still a plaintext secret.
    let scratch = Scratch::new("gate-unsealed");
    let path = scratch.config();
    fs::write(
        &path,
        serde_json::to_string_pretty(&json!({ "credentials.api_key": "sk-plain-123456" }))
            .expect("serialize"),
    )
    .expect("write a legacy file");

    let error = load_overlay_file(&path).expect_err("a plaintext credential must not load");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("credentials.api_key"),
        "the message names the dot-path: {}",
        error.message
    );
}

#[test]
fn a_file_that_is_not_json_is_refused_and_names_the_parse() {
    let scratch = Scratch::new("gate-bad-json");
    let path = scratch.config();
    fs::write(&path, "{ not json").expect("write a broken file");

    let error = load_overlay_file(&path).expect_err("a broken file must not load");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("parse error"),
        "the message names the parse: {}",
        error.message
    );
}

#[test]
fn a_path_that_cannot_be_read_is_refused() {
    let scratch = Scratch::new("gate-unreadable");
    let path = scratch.config();
    fs::create_dir(&path).expect("a directory at the config path");

    let error = load_overlay_file(&path).expect_err("a directory cannot be read as a file");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("failed to read"),
        "the message names the read: {}",
        error.message
    );
}
