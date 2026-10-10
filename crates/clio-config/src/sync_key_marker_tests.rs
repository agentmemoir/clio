// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the recorded store choice: that the
//! marker round-trips, that an unreadable one is reported rather than guessed,
//! and that generation records the choice before it writes any key byte.
//!
//! ## Owns
//! - Every test of `record_source`, `recorded_source`, and the marker's part of
//!   `generate`
//!
//! ## Does not own
//! - The generate, load, and report rules around the key bytes (see
//!   `sync_key_tests`)
//! - The environment-driven wrappers (see `sync_key_env_tests`)
//! - The marker's own read and write rules (see `secret_key_source_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every test injects the
//! environment and points the marker at a self-cleaning temp directory, so no
//! test reaches the process environment or the machine's real data directory.
//!
//! Keep this module focused on the recorded store choice.

use super::*;
use crate::secret_key::KEY_LEN;
use crate::secret_key_source::{MARKER_NAME, SYNC_MARKER_NAME, sync_marker_path};
use crate::sync_key_test_support::{TempDir, env};

/// The marker functions read the path through the injected environment, so the
/// tests drive them with an environment that names the temp file.
fn record_source_for(dir: &TempDir, source: KeySource) -> Result<KeySource, AmError> {
    let path = dir.key().display().to_string();
    let get = env(vec![(SYNC_KEY_FILE_ENV, path)]);
    record_source(&get, source)
}

fn recorded_source_for(dir: &TempDir) -> Result<Option<KeySource>, AmError> {
    let path = dir.key().display().to_string();
    let get = env(vec![(SYNC_KEY_FILE_ENV, path)]);
    recorded_source(&get)
}

/// Seed a marker naming a store Clio does not know.
fn seed_unknown_store(dir: &TempDir) -> PathBuf {
    let marker = sync_marker_path(Some(&dir.key())).expect("a key file has a directory");
    std::fs::write(&marker, "smartcard\n").expect("seed a record Clio cannot read");
    marker
}

#[test]
fn the_recorded_source_round_trips_and_an_absent_marker_means_file() {
    let dir = TempDir::new("marker");

    assert_eq!(
        recorded_source_for(&dir).expect("read"),
        None,
        "nothing recorded yet"
    );
    assert_eq!(
        record_source_for(&dir, KeySource::Keystore).expect("record"),
        KeySource::Keystore
    );
    assert_eq!(
        recorded_source_for(&dir).expect("read"),
        Some(KeySource::Keystore)
    );
    assert_eq!(
        recorded_source_for(&TempDir::new("marker-absent")).expect("read"),
        None,
        "an absent record means the file store, which is what a load then reads"
    );
}

#[test]
fn the_wizard_reader_reports_a_marker_it_cannot_read() {
    // The wizard calls this to decide whether to ask the question. A record that
    // exists but cannot be read has to be reported: read as "no choice" it would
    // ask again and then fail to record the answer, so the operator would answer
    // a question the machine had already answered.
    let dir = TempDir::new("marker-unreadable");
    seed_unknown_store(&dir);

    let err = recorded_source_for(&dir).expect_err("an unknown store is reported");

    assert!(err.message.contains("smartcard"), "{}", err.message);
}

#[test]
fn the_recorded_choice_lives_in_the_sync_keys_own_marker() {
    let dir = TempDir::new("marker-own-file");
    record_source_for(&dir, KeySource::File).expect("record");

    assert!(
        dir.path().join(SYNC_MARKER_NAME).exists(),
        "the sync key's choice is its own file, never the data key's"
    );
    assert!(
        !dir.path().join(MARKER_NAME).exists(),
        "recording the sync key's store must not write the data key's record"
    );
}

#[test]
fn a_marker_naming_an_unknown_store_is_refused_rather_than_defaulted() {
    let dir = TempDir::new("marker-corrupt");
    seed_unknown_store(&dir);
    let path = dir.key().display().to_string();
    let get = env(vec![(SYNC_KEY_FILE_ENV, path)]);

    let err = load(&get).expect_err("an unknown store is refused, never defaulted");
    assert!(err.message.contains("smartcard"), "{}", err.message);
    let err = report(&get).expect_err("the report follows the same rule");
    assert!(err.message.contains("smartcard"), "{}", err.message);
}

#[test]
fn a_recorded_store_disagreeing_with_the_flag_is_refused() {
    let dir = TempDir::new("marker-disagree");
    record_source_for(&dir, KeySource::File).expect("record");
    let path = dir.key().display().to_string();
    let get = env(vec![(SYNC_KEY_FILE_ENV, path)]);

    let err = generate(&get, Some(KeySource::Keystore)).expect_err("a disagreement is refused");

    assert!(err.message.contains("already recorded"), "{}", err.message);
}

#[test]
fn a_rotated_generation_still_records_the_store_choice() {
    // Rotation replaced the refusal: a key that already exists is archived and
    // a new one written, and the record still names the store the key went to,
    // because every later load resolves the store from that record alone.
    let dir = TempDir::new("marker-rotated");
    crate::sync_key_file::write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("seed a key file");
    let path = dir.key().display().to_string();
    let get = env(vec![(SYNC_KEY_FILE_ENV, path)]);

    generate(&get, Some(KeySource::File)).expect("an existing key is rotated, not refused");

    assert_eq!(
        recorded_source(&get)
            .expect("read the record back")
            .expect("the store choice is recorded"),
        KeySource::File,
        "the record names the store the rotated key was written to"
    );
}

#[test]
fn a_refused_rotation_records_no_store_choice() {
    // The invariant: a record written on the way to a refusal names a store no
    // key ever reached. The shadowing refusal runs before the record is
    // confirmed, so a refused run leaves the machine exactly as it found it.
    let dir = TempDir::new("marker-refused-rotation");
    crate::sync_key_file::write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("seed a shadowing file");
    let path = dir.key().display().to_string();
    let get = env(vec![(SYNC_KEY_FILE_ENV, path)]);

    let err = generate(&get, Some(KeySource::Keystore))
        .expect_err("a second copy of the same secret is refused");

    assert!(err.message.contains("already exists"), "{}", err.message);
    assert!(dir.key().exists(), "a refused rotation touches nothing");
    assert_eq!(
        recorded_source(&get).expect("read"),
        None,
        "the refusal came before any record was written"
    );
}

#[test]
fn a_successful_generation_records_the_store_the_key_went_to() {
    // The guard for the record write itself: `generate` is the only caller that
    // records without being asked to, so a dropped call would leave every load
    // reading the default store whatever `--store` said.
    let dir = TempDir::new("marker-recorded");
    let path = dir.key().display().to_string();
    let get = env(vec![(SYNC_KEY_FILE_ENV, path)]);

    generate(&get, Some(KeySource::File)).expect("generate");

    assert_eq!(
        recorded_source(&get).expect("read"),
        Some(KeySource::File),
        "the record names the store the key was written to"
    );
}

#[test]
fn generation_records_the_store_before_it_writes_a_key() {
    // The marker is the record every reader resolves the store from, so writing
    // the key first would leave bytes in a store nothing will look in, and the
    // refusal would send the operator back to `keygen` for a second key.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new("marker-first");
        symlink(
            dir.path().join("missing-target"),
            dir.path().join(SYNC_MARKER_NAME),
        )
        .expect("the marker path is a dangling symlink");
        let path = dir.key().display().to_string();
        let get = env(vec![(SYNC_KEY_FILE_ENV, path)]);

        let err = generate(&get, Some(KeySource::File)).expect_err("the record cannot be written");

        assert!(
            err.message.contains("key-store record"),
            "the refusal names the record that could not be written: {}",
            err.message
        );
        assert!(
            !dir.key().exists(),
            "no key is written when the store choice could not be recorded"
        );
    }
}
