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
//!
//! ## Does not own
//! - Production save logic under test
//! - The overwrite check itself (see `load_tests`)
//! - The shared atomic writer (see `write_tests`)
//!
//! ## Boundary
//!
//! Changes to what the checked save accepts or writes should normally update
//! tests here. Changes to the check alone should not.
//!
//! Keep this module focused on the checked save.

use super::super::load::check_overwritable;
use super::*;
use std::path::PathBuf;

/// A scratch directory that cleans itself up when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "am-save-{tag}-{}-{nanos}",
            std::process::id()
        ));
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
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
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
