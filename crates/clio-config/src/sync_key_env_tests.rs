// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the environment-driven entry points of
//! the sync key lifecycle: the wrappers that pick the store, resolve the path,
//! and return either a key or a report.
//!
//! ## Owns
//! - Every test of `load`, `load_base64`, `generate`, `delete`, `report`, and
//!   `record_source` through an injected environment
//! - The empty-store and caller-supplied-path branches of the `_with` functions
//!
//! ## Does not own
//! - The store-free lifecycle rules (see `sync_key_tests`)
//! - The file and sidecar internals (see `sync_key_file_tests`)
//!
//! ## Boundary
//!
//! Test-only. Every test injects the environment and points the file store at a
//! self-cleaning temp directory. The one keystore path exercised here is the
//! refusal that returns before the OS store is touched.
//!
//! Keep this module focused on the wrappers.

use super::*;
use crate::secret_key::KEY_LEN;
use crate::secret_key_store_test_support::{FakeCredential, store_over_credential};
use crate::sync_key_test_support::{TempDir, env_of};

/// An injected environment whose only value is the sync key file path.
fn file_env(dir: &TempDir) -> impl Fn(&str) -> Option<String> + use<> {
    let path = dir.key().display().to_string();
    move |key: &str| (key == SYNC_KEY_FILE_ENV).then(|| path.clone())
}

#[test]
fn the_env_driven_lifecycle_generates_loads_reports_and_deletes() {
    let dir = TempDir::new("lifecycle");
    let env = file_env(&dir);

    let generated = generate(&env, Some(KeySource::File)).expect("generate");
    assert_eq!(
        record_source(&env, KeySource::File).expect("record"),
        KeySource::File,
        "the second call confirms the record rather than rewriting it"
    );

    let loaded = load(&env).expect("load").expect("a key exists");
    assert_eq!(loaded.expose(), generated.expose());
    assert_eq!(load_base64(&env).expect("base64"), Some(generated.base64()));

    let first = report(&env).expect("report");
    assert_eq!(first.store, KeySource::File);
    assert_eq!(first.count, 1);
    assert_eq!(
        first.fingerprint.as_deref(),
        Some(generated.fingerprint().as_str())
    );

    delete(&env).expect("delete");
    assert!(
        load(&env).expect("load after delete").is_none(),
        "the key is gone"
    );
    delete(&env).expect("deleting nothing is the state the caller asked for");

    let after = report(&env).expect("report after delete");
    assert_eq!(after.count, 0);
    assert_eq!(after.fingerprint, None);
}

#[test]
fn generation_without_a_flag_uses_the_recorded_choice_or_the_default() {
    let recorded = TempDir::new("recorded");
    let env = file_env(&recorded);
    record_source(&env, KeySource::File).expect("record");

    let from_record = generate(&env, None).expect("a recorded choice drives generation");
    assert_eq!(from_record.location, SyncKeyLocation::File(recorded.key()));

    let fresh = TempDir::new("default");
    let env = file_env(&fresh);
    let from_default = generate(&env, None).expect("the recommended store answers");
    assert_eq!(from_default.location, SyncKeyLocation::File(fresh.key()));
}

#[test]
fn recording_a_choice_with_no_resolvable_path_is_refused() {
    let err = record_source(&|_| None, KeySource::File).expect_err("nowhere to record");

    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
    assert!(
        err.message.contains("nowhere to be recorded"),
        "{}",
        err.message
    );
}

#[test]
fn a_keystore_generation_with_no_resolvable_marker_path_writes_nothing() {
    // A bare relative `CLIO_KEY_FILE` resolves the key file, so the file store
    // has somewhere to go, but the record is derived from the key file's
    // directory and a bare name has none. Before the order was reversed the key
    // reached the keystore and every reader resolved the store from the record
    // alone, found the file store instead, and reported no key at all.
    let get = env_of(&[("CLIO_KEY_FILE", "clio.key")]);
    let key_file = crate::sync_key_file::key_file_path(&get);
    let marker = crate::secret_key_source::sync_marker_path(key_file.as_deref());

    let err = generate(&get, Some(KeySource::Keystore)).expect_err("the record cannot be written");

    assert!(
        marker.is_none(),
        "the premise: a bare relative key file has no directory to record in"
    );
    assert!(
        err.message.contains("nowhere to be recorded"),
        "the refusal names the record that has nowhere to go: {}",
        err.message
    );
}

#[test]
fn every_caller_supplied_path_branch_handles_a_missing_path() {
    assert!(
        load_with(&NoKeystore, KeySource::File, None)
            .expect("no path is not an error")
            .is_none()
    );
    assert_eq!(
        report_with(&NoKeystore, KeySource::File, None)
            .expect("no path is not an error")
            .count,
        0
    );
    delete_with(&NoKeystore, KeySource::File, None).expect("nothing to remove");
    let err = generate_with(&NoKeystore, KeySource::File, None).expect_err("nowhere to write");
    assert!(
        err.message.contains("no key-file path could be resolved"),
        "{}",
        err.message
    );
}

#[test]
fn an_empty_keystore_reports_no_key_and_deletes_successfully() {
    let store = store_over_credential(SYNC_ACCOUNT, SYNC_KEY, FakeCredential::empty());

    assert!(
        load_with(&store, KeySource::Keystore, None)
            .expect("reachable and empty")
            .is_none()
    );
    assert_eq!(
        report_with(&store, KeySource::Keystore, None)
            .expect("reachable and empty")
            .count,
        0
    );
    delete_with(&store, KeySource::Keystore, None).expect("deleting nothing");
}

#[test]
fn a_refused_keystore_generation_leaves_the_key_file_and_the_record_alone() {
    // The shape an operator reaches by following the refusal's own advice: the
    // record is gone, the key file is not, and the flag now names the other store.
    // Recording first would write a keystore record here, so every later load and
    // report would resolve the keystore, find no key, and refuse the file that
    // holds the only one.
    let dir = TempDir::new("keystore-file-shadow");
    crate::sync_key_file::write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("seed the file key");
    let env = file_env(&dir);

    // The refusal happens on the shadowing file, so the production keystore this
    // builds is never read or written.
    let err = generate(&env, Some(KeySource::Keystore)).expect_err("a file key exists");

    assert!(err.message.contains("already exists"), "{}", err.message);
    assert!(err.message.contains("key file"), "{}", err.message);
    assert!(
        recorded_source(&env)
            .expect("read the record back")
            .is_none(),
        "the refused run left no record to resolve the store from"
    );
}
