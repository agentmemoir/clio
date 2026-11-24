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
//!
//! ## Does not own
//! - Production load logic under test
//! - The save side that uses the guard (see `save_tests`)
//!
//! ## Boundary
//!
//! Changes to what the overwrite check accepts should normally update tests
//! here. Changes to what a save writes should not.
//!
//! Keep this module focused on the overwrite check.

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
