// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the value the sync key lifecycle hands
//! back: the loaded key's disclosure rules, its base64 form, the fingerprint,
//! and the report helpers.
//!
//! ## Owns
//! - The fingerprint's stability, difference, width, and non-prefix property
//! - The base64 form and the masked `Debug`
//!
//! ## Does not own
//! - Generation, storage, and deletion (see `sync_key_tests`)
//! - The key file and sidecar (see `sync_key_file_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every key here is
//! generated in the test or drawn from the thread RNG; no real key is stored.
//!
//! Keep this module focused on the value's derivations.

use super::*;

#[test]
fn the_fingerprint_is_stable_for_one_key_and_different_for_another() {
    let first = LoadedSyncKey::new(Zeroizing::new([1_u8; KEY_LEN]), SyncKeyLocation::Keystore);
    let same = LoadedSyncKey::new(
        Zeroizing::new([1_u8; KEY_LEN]),
        SyncKeyLocation::File("/x".into()),
    );
    let second = LoadedSyncKey::new(Zeroizing::new([2_u8; KEY_LEN]), SyncKeyLocation::Keystore);

    assert_eq!(first.fingerprint(), same.fingerprint());
    assert_ne!(first.fingerprint(), second.fingerprint());
    assert_eq!(first.fingerprint().len(), 16, "a comparable width");
}

#[test]
fn the_fingerprint_is_not_a_prefix_of_the_key_in_any_printed_form() {
    let key = rand::random::<[u8; KEY_LEN]>();
    let loaded = LoadedSyncKey::new(Zeroizing::new(key), SyncKeyLocation::Keystore);

    let fingerprint = loaded.fingerprint();

    assert!(!loaded.base64().starts_with(&fingerprint));
    assert!(!hex_of(&key).starts_with(&fingerprint));
}

#[test]
fn the_fingerprint_is_sixteen_lowercase_hex_characters() {
    let loaded = LoadedSyncKey::new(
        Zeroizing::new(rand::random::<[u8; KEY_LEN]>()),
        SyncKeyLocation::Keystore,
    );

    let fingerprint = loaded.fingerprint();

    assert_eq!(fingerprint.len(), 16);
    assert!(fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(fingerprint.chars().all(|c| !c.is_ascii_uppercase()));
}

#[test]
fn printing_a_loaded_key_never_prints_its_bytes() {
    let key = rand::random::<[u8; KEY_LEN]>();
    let loaded = LoadedSyncKey::new(Zeroizing::new(key), SyncKeyLocation::Keystore);

    let rendered = format!("{loaded:?}");

    assert!(rendered.contains("<redacted>"), "{rendered}");
    assert!(!rendered.contains(&hex_of(&key)), "{rendered}");
    assert!(!rendered.contains(&base64::engine::general_purpose::STANDARD.encode(key)));
}

#[test]
fn the_base64_form_is_standard_base64_of_the_key_bytes() {
    let key = rand::random::<[u8; KEY_LEN]>();
    let loaded = LoadedSyncKey::new(Zeroizing::new(key), SyncKeyLocation::Keystore);

    let encoded = loaded.base64();

    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .expect("valid base64"),
        key.to_vec()
    );
}

/// Lower-case hex, the encoding a rendered value would most plausibly leak in.
fn hex_of(key: &[u8; KEY_LEN]) -> String {
    use std::fmt::Write as _;
    key.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}
