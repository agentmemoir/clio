// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the checked save in
//! `config::persist::save`.
//!
//! ## Owns
//! - A save that refuses a deployment file the check did not judge
//! - A save that replaces the file it judged
//! - No temp file left after a refusal
//! - The stored bytes of a save: a secret value sealed, an ordinary one plain,
//!   and a sealed value that opens back after a reload
//!
//! ## Does not own
//! - Production save logic under test
//! - The overwrite check itself (see `load_tests`)
//! - The load-side refusal shapes (see `seal_gate_tests`)
//! - The shared atomic writer (see `write_tests`)
//!
//! ## Boundary
//!
//! Changes to what the checked save accepts or writes should normally update
//! tests here. Changes to the check alone should not.
//!
//! Keep this module focused on the save and the bytes it writes.

use super::super::load::check_overwritable;
use super::*;
use crate::secret_seal::test_key;
use std::path::PathBuf;

/// The key the sealing tests here write and read under.
const KEY: [u8; crate::secret_key::KEY_LEN] = [0x5a; crate::secret_key::KEY_LEN];

/// A URL whose userinfo carries the password a user pasted into it.
const USERINFO_URL: &str = "https://user:DUMMY_PASSWORD@host";

/// The JSON the save wrote to `path`.
fn saved_file(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read the saved file"))
        .expect("the saved file is JSON")
}

/// A scratch directory that cleans itself up when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("am-save-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn config(&self) -> PathBuf {
        self.0.join("deployment.json")
    }

    /// The entry names in the scratch directory, sorted so the assertion does
    /// not depend on `read_dir` order.
    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.0)
            .expect("read_dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn overlay(pairs: &[(&str, &str)]) -> Overlay {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Value::String((*value).to_owned())))
        .collect()
}

#[cfg(unix)]
#[test]
fn checked_save_refuses_a_file_swapped_after_the_check() {
    let scratch = Scratch::new("swapped");
    let path = scratch.config();
    fs::write(&path, r#"{"log.level":"info"}"#).expect("seed");
    let guard = check_overwritable(&path).expect("a loadable file passes");

    let swapped = "{ not json";
    let other = scratch.0.join("other.json");
    fs::write(&other, swapped).expect("swap");
    fs::rename(&other, &path).expect("replace the entry");

    let err = save_overlay_file_checked(&path, &overlay(&[("log.level", "warn")]), &guard)
        .expect_err("the swapped file is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(
        err.message.contains("changed while Clio checked it"),
        "the refusal names the change under check: {}",
        err.message
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        swapped,
        "the swapped file must keep its bytes"
    );
    assert_eq!(
        scratch.entries(),
        vec!["deployment.json".to_owned()],
        "a refusal must remove its temp file"
    );
}

#[test]
fn checked_save_replaces_the_file_it_judged() {
    let scratch = Scratch::new("normal");
    let path = scratch.config();
    fs::write(&path, r#"{"log.level":"info"}"#).expect("seed");
    let guard = check_overwritable(&path).expect("a loadable file passes");

    save_overlay_file_checked(&path, &overlay(&[("log.level", "warn")]), &guard)
        .expect("the unchanged path is replaced");

    let stored: Value = serde_json::from_str(&fs::read_to_string(&path).expect("read"))
        .expect("the saved file is JSON");
    assert_eq!(stored["log.level"], Value::String("warn".to_owned()));
    assert_eq!(
        scratch.entries(),
        vec!["deployment.json".to_owned()],
        "the renamed temp file must not outlive the write"
    );
}

#[test]
fn checked_save_refuses_a_file_that_appears_after_an_absent_check() {
    let scratch = Scratch::new("absent");
    let path = scratch.config();
    let guard = check_overwritable(&path).expect("a missing file passes");

    let appeared = r#"{"log.level":"debug"}"#;
    fs::write(&path, appeared).expect("appear");

    let err = save_overlay_file_checked(&path, &overlay(&[("log.level", "warn")]), &guard)
        .expect_err("a file created under the check is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        appeared,
        "the file that appeared must keep its bytes"
    );
    assert_eq!(
        scratch.entries(),
        vec!["deployment.json".to_owned()],
        "a refusal must remove its temp file"
    );
}

#[test]
fn checked_save_creates_the_file_when_the_path_stays_absent() {
    let scratch = Scratch::new("create");
    let path = scratch.config();
    let guard = check_overwritable(&path).expect("a missing file passes");

    save_overlay_file_checked(&path, &overlay(&[("log.level", "warn")]), &guard)
        .expect("an absent path is created");

    let stored: Value = serde_json::from_str(&fs::read_to_string(&path).expect("read"))
        .expect("the saved file is JSON");
    assert_eq!(stored["log.level"], Value::String("warn".to_owned()));
}

#[cfg(unix)]
#[test]
fn checked_save_refuses_a_symlink_entry_and_leaves_its_target() {
    let scratch = Scratch::new("symlink");
    let path = scratch.config();
    let target = scratch.0.join("real.json");
    fs::write(&target, r#"{"log.level":"info"}"#).expect("target");
    std::os::unix::fs::symlink(&target, &path).expect("symlink");
    let guard = check_overwritable(&path).expect("the loader follows the link");

    let err = save_overlay_file_checked(&path, &overlay(&[("log.level", "warn")]), &guard)
        .expect_err("a link entry is refused");

    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(
        err.message.contains("symbolic link"),
        "the refusal names the link: {}",
        err.message
    );
    assert!(
        fs::symlink_metadata(&path)
            .expect("meta")
            .file_type()
            .is_symlink(),
        "the link entry must still be a link"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read"),
        r#"{"log.level":"info"}"#,
        "the link target must keep its bytes"
    );
}

#[test]
fn a_url_with_userinfo_is_sealed_at_a_path_that_is_not_sealed() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("userinfo");
    let path = scratch.config();

    save_overlay_file(&path, &overlay(&[("extract.url", USERINFO_URL)])).expect("save");

    let text = fs::read_to_string(&path).expect("read");
    assert!(
        !text.contains("DUMMY_PASSWORD"),
        "the password reached the file: {text}"
    );
    let Value::String(sealed) = saved_file(&path)["extract.url"].clone() else {
        panic!("the stored value is a string");
    };
    assert!(
        crate::secret_seal::is_sealed(&sealed),
        "the stored value is an envelope: {sealed}"
    );
}

#[test]
fn a_plain_value_at_a_path_that_is_not_sealed_stays_plaintext() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("plain");
    let path = scratch.config();

    save_overlay_file(
        &path,
        &overlay(&[("extract.url", "https://host/v1"), ("log.level", "debug")]),
    )
    .expect("save");

    let saved = saved_file(&path);
    assert_eq!(saved["extract.url"], Value::from("https://host/v1"));
    assert_eq!(saved["log.level"], Value::from("debug"));
}

#[test]
fn a_saved_url_with_userinfo_opens_back_after_a_reload() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("userinfo-round-trip");
    let path = scratch.config();

    save_overlay_file(
        &path,
        &overlay(&[("extract.url", USERINFO_URL), ("log.level", "debug")]),
    )
    .expect("save");
    let reloaded =
        super::super::load_overlay_file(&path).expect("an envelope at an ordinary path reloads");

    assert_eq!(
        reloaded.get("extract.url"),
        Some(&Value::from(USERINFO_URL)),
        "the round trip returns the value that was set"
    );
    assert_eq!(reloaded.get("log.level"), Some(&Value::from("debug")));
}

#[test]
fn saving_an_overlay_that_already_holds_an_envelope_does_not_seal_it_again() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("envelope-passthrough");
    let path = scratch.config();
    save_overlay_file(&path, &overlay(&[("extract.url", USERINFO_URL)])).expect("first save");
    let Value::String(envelope) = saved_file(&path)["extract.url"].clone() else {
        panic!("the stored value is a string");
    };

    save_overlay_file(&path, &overlay(&[("extract.url", &envelope)])).expect("second save");

    assert_eq!(
        saved_file(&path)["extract.url"],
        Value::from(envelope),
        "an envelope is written through rather than wrapped a second time"
    );
    assert_eq!(
        super::super::load_overlay_file(&path)
            .expect("reload")
            .get("extract.url"),
        Some(&Value::from(USERINFO_URL)),
        "one open returns the value the user set"
    );
}

#[test]
fn a_non_string_value_keeps_its_type_and_its_path() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("non-string");
    let path = scratch.config();
    let mut pairs = overlay(&[("extract.url", USERINFO_URL)]);
    pairs.insert("extract.temperature".into(), Value::from(0.5));

    save_overlay_file(&path, &pairs).expect("save");

    let saved = saved_file(&path);
    assert_eq!(saved["extract.temperature"], Value::from(0.5));
    assert!(
        !serde_json::to_string(&saved["extract.url"])
            .expect("serialize")
            .contains("DUMMY_PASSWORD"),
        "the string beside it is still sealed"
    );
}

#[test]
fn save_reports_a_parent_that_cannot_be_created() {
    let scratch = Scratch::new("blocked-parent");
    let blocker = scratch.0.join("blocker");
    fs::write(&blocker, "x").expect("blocker");

    let err = save_overlay_file(
        &blocker.join("child.json"),
        &overlay(&[("log.level", "debug")]),
    )
    .expect_err("a file cannot hold a directory");

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message.contains("failed to create"),
        "the message names the step that failed: {}",
        err.message
    );
}

#[test]
fn save_reports_a_write_that_fails() {
    let scratch = Scratch::new("write-fails");
    let path = scratch.config();
    fs::create_dir(&path).expect("a directory at the config path");

    let err = save_overlay_file(&path, &overlay(&[("log.level", "debug")]))
        .expect_err("a directory cannot be replaced by a file");

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message.contains("failed to write deployment config"),
        "the message names the step that failed: {}",
        err.message
    );
}
