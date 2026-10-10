// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the recorded key-store choice: that it
//! round-trips, that it is read once and never rewritten, and that an absent
//! record means the recommended store rather than an error.
//!
//! ## Owns
//! - Every test of `KeySource` parsing, defaulting, recording, and reading
//!
//! ## Does not own
//! - The key bytes and the two stores (see `secret_key_tests`)
//! - The scaffolding these tests stand on (see `secret_key_support_tests`)
//!
//! ## Boundary
//!
//! A test here asserts one fact about the record, named for the fact it pins.
//! Adding a test means adding a case that would fail if that rule changed.
//!
//! Keep this module focused on the record of the choice.

use super::{KeySource, MARKER_NAME, marker_path, read, write};
use std::fs;
use std::path::{Path, PathBuf};

/// A directory that is unique per call and removed when the guard drops.
///
/// Self-cleaning so a test run leaves nothing behind, which is the one thing the
/// shared scaffolding cannot promise.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "clio-key-source-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("the temp directory can be created");
        Self(path)
    }

    fn marker(&self) -> PathBuf {
        self.0.join(MARKER_NAME)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn both_stores_round_trip_through_the_marker() {
    for source in KeySource::ALL {
        let dir = TempDir::new("round-trip");
        write(Some(&dir.marker()), source).expect("the record is written");
        let found = read(Some(&dir.marker())).expect("the record is readable");
        assert_eq!(found, Some(source), "after recording {source}");
    }
}

#[test]
fn the_record_holds_exactly_the_store_name_and_a_newline() {
    let dir = TempDir::new("contents");
    write(Some(&dir.marker()), KeySource::Keystore).expect("the record is written");
    let raw = fs::read_to_string(dir.marker()).expect("the record is readable");
    assert_eq!(raw, "keystore\n", "the record is a whole-word name");
}

#[test]
fn an_absent_record_reads_as_no_choice_rather_than_an_error() {
    let dir = TempDir::new("absent");
    let found = read(Some(&dir.marker())).expect("an absent record is not a failure");
    assert_eq!(found, None, "nothing recorded means nothing chosen");
}

#[test]
fn an_absent_marker_path_reads_as_no_choice() {
    let found = read(None).expect("no path is not a failure");
    assert_eq!(found, None, "nowhere to record means nothing chosen");
}

#[test]
fn a_record_naming_an_unknown_store_is_refused_rather_than_guessed() {
    let dir = TempDir::new("unknown");
    fs::write(dir.marker(), "smartcard\n").expect("the record is written");
    let error = read(Some(&dir.marker())).expect_err("an unknown store is refused");
    assert!(
        error.message.contains("smartcard"),
        "the error names what it read: {}",
        error.message
    );
    assert!(
        error.message.contains("file|keystore"),
        "the error lists what it accepts: {}",
        error.message
    );
}

#[test]
fn a_record_is_never_rewritten() {
    let dir = TempDir::new("once");
    write(Some(&dir.marker()), KeySource::File).expect("the first record is written");
    // `create_new` is what enforces this, so the second write has to fail rather
    // than replace what a run already decided.
    let error = write(Some(&dir.marker()), KeySource::Keystore)
        .expect_err("a recorded choice is not overwritten");
    assert!(
        error.message.contains("create"),
        "the error names the file it would have replaced: {}",
        error.message
    );
    let found = read(Some(&dir.marker())).expect("the original record still reads");
    assert_eq!(found, Some(KeySource::File), "the first choice stands");
}

#[test]
fn the_record_sits_beside_the_key_file() {
    let key_file = Path::new("/var/lib/clio/clio.key");
    let marker = marker_path(Some(key_file)).expect("a key file has a directory");
    assert_eq!(marker, Path::new("/var/lib/clio").join(MARKER_NAME));
}

#[test]
fn a_key_file_with_no_directory_has_no_marker_path() {
    assert_eq!(
        marker_path(Some(Path::new("clio.key"))),
        None,
        "a bare filename has nowhere to put a record"
    );
}

#[test]
fn the_recommended_store_is_the_one_that_works_without_an_os_secret_store() {
    assert_eq!(
        KeySource::recommended(),
        KeySource::File,
        "the default must be the store a headless machine can actually use"
    );
}

#[test]
fn every_store_name_parses_case_insensitively_and_tolerates_padding() {
    for source in KeySource::ALL {
        let name = source.as_str();
        assert_eq!(KeySource::parse(name), Some(source), "exact {name}");
        assert_eq!(
            KeySource::parse(&name.to_uppercase()),
            Some(source),
            "uppercase {name}"
        );
        assert_eq!(
            KeySource::parse(&format!("  {name}  ")),
            Some(source),
            "padded {name}"
        );
    }
}

#[test]
fn an_unknown_name_parses_to_nothing_rather_than_to_a_default() {
    assert_eq!(
        KeySource::parse("keychain"),
        None,
        "a Mac-only word is not a store name Clio records"
    );
}

#[test]
fn the_accepted_names_list_every_store() {
    assert_eq!(KeySource::accepted(), "file|keystore");
}
