// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the secret envelope: the round trip,
//! the seal-side passthrough, the strict refusal of unsealed input, and every
//! way a stored value can fail to open.
//!
//! ## Owns
//! - Round-trip, stability, and no-leak assertions for [`super::seal`] and
//!   [`super::open`]
//! - The failure cases: altered envelope, wrong key, truncated, not base64,
//!   not sealed at all
//! - The sealed-path list
//!
//! ## Does not own
//! - The key's own lifecycle (see `secret_key_report_tests`)
//! - Writing and reading a config file around a sealed value (see
//!   `config`'s `persist_seal_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on the envelope. Every test here passes an explicit
//! key, so none of them reads the process cache or the machine's key store.

use super::{AAD, KEY_LEN, SEALED_PREFIX, is_sealed, is_sealed_path, open, seal};
use clio_types::ErrorCode;

/// The key most tests seal under. Any distinct 32 bytes work.
const KEY: [u8; KEY_LEN] = [0x11; KEY_LEN];

/// A second key, for the wrong-key case.
const OTHER_KEY: [u8; KEY_LEN] = [0x22; KEY_LEN];

/// Values that must survive a round trip byte for byte.
fn round_trip_cases() -> Vec<(&'static str, String)> {
    vec![
        ("empty", String::new()),
        ("ascii", "sk-live-abcdef123456".to_owned()),
        ("non-ascii", "clé-de-séance-日本語-🔑".to_owned()),
        ("newline", "line one\nline two\r\nline three".to_owned()),
        ("four KiB", "x".repeat(4096)),
    ]
}

#[test]
fn every_case_opens_back_to_the_original_bytes() {
    for (name, value) in round_trip_cases() {
        let sealed = seal_with(&value);
        assert_eq!(open_with(&sealed), Ok(value.clone()), "{name}");
    }
}

#[test]
fn sealing_the_same_value_twice_is_stable() {
    let once = seal_with("sk-stability-check");
    let twice = seal_with(&once);
    assert_eq!(twice, once, "an already-sealed value is returned unchanged");
    assert_eq!(open_with(&once), Ok("sk-stability-check".to_owned()));
}

#[test]
fn two_seals_of_one_value_differ() {
    // A fresh nonce per seal. Identical envelopes would mean a reused nonce,
    // which leaks the XOR of two plaintexts.
    let first = seal_with("sk-nonce-check");
    let second = seal_with("sk-nonce-check");
    assert_ne!(first, second);
    assert_eq!(open_with(&first), open_with(&second));
}

#[test]
fn the_stored_form_reveals_nothing_about_the_secret() {
    let secret = "sk-live-0123456789abcdefghijklmnopqrstuvwxyz";
    let sealed = seal_with(secret);

    assert!(!sealed.contains(secret), "the plaintext is in the envelope");
    assert!(
        !sealed.contains(&secret[..8]),
        "the head of the secret is there"
    );
    assert!(
        !sealed.contains(&secret[secret.len() - 8..]),
        "the tail of the secret is there"
    );
    // The marker and one base64 body, and nothing else.
    let body = sealed.strip_prefix(SEALED_PREFIX).expect("prefix");
    assert!(!body.contains(' '), "the body is not readable text");
    assert!(
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body).is_ok(),
        "the body is standard base64"
    );
}

#[test]
fn a_blank_value_is_still_sealed() {
    // A stored blank API key must not be distinguishable from a real one by
    // looking at the file.
    let sealed = seal_with("");
    assert!(is_sealed(&sealed));
    assert_eq!(open_with(&sealed), Ok(String::new()));
}

#[test]
fn an_altered_ciphertext_character_fails_to_open() {
    let sealed = seal_with("sk-tamper-check-0123456789");
    let mut chars: Vec<char> = sealed.chars().collect();
    // Flip a character inside the base64 body, past the prefix and past the
    // nonce, so the failure is authentication and not a length check.
    let index = SEALED_PREFIX.len() + 20;
    let original = chars[index];
    chars[index] = if original == 'A' { 'B' } else { 'A' };
    let altered: String = chars.into_iter().collect();

    let error = open_with(&altered).expect_err("an altered envelope must not open");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("authentication"),
        "the message names authentication: {}",
        error.message
    );
    assert!(
        !error.message.contains("sk-tamper-check"),
        "the message never echoes the value: {}",
        error.message
    );
}

#[test]
fn an_altered_nonce_fails_to_open() {
    let sealed = seal_with("sk-nonce-tamper");
    let mut chars: Vec<char> = sealed.chars().collect();
    let index = SEALED_PREFIX.len() + 2;
    chars[index] = if chars[index] == 'A' { 'B' } else { 'A' };
    let altered: String = chars.into_iter().collect();

    assert!(
        open_with(&altered).is_err(),
        "an altered nonce must not open"
    );
}

#[test]
fn a_value_sealed_under_another_key_does_not_open() {
    let sealed = seal_under(&OTHER_KEY, "sk-wrong-key-check");

    let error = open_with(&sealed).expect_err("another key's envelope must not open");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("different Clio encryption key"),
        "the message names the likely cause: {}",
        error.message
    );
}

#[test]
fn a_prefix_with_nothing_after_it_fails_with_a_message() {
    let error = open_with(SEALED_PREFIX).expect_err("an empty body must not open");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(error.message.contains("truncated"), "{}", error.message);
}

#[test]
fn a_prefix_with_a_truncated_payload_fails_with_a_message() {
    // A nonce and no ciphertext: shorter than the tag alone.
    let short = format!(
        "{SEALED_PREFIX}{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0_u8; 4])
    );
    assert!(open_with(&short).is_err());

    // A full nonce and a ciphertext too short to carry an authentication tag.
    let mut bytes = vec![0_u8; super::NONCE_LEN];
    bytes.extend_from_slice(&[1, 2, 3, 4]);
    let barely = format!(
        "{SEALED_PREFIX}{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    );
    let error = open_with(&barely).expect_err("a ciphertext with no tag must not open");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("authentication"),
        "the message names authentication rather than panicking: {}",
        error.message
    );
}

#[test]
fn a_body_that_is_not_base64_fails_with_a_message() {
    for body in ["!!!!", "not base64 at all", "====", "sealed:v1:"] {
        let error = open_with(body).expect_err("a non-base64 body must not open");
        assert_eq!(error.code, ErrorCode::ConfigCorrupt, "{body}");
        assert!(!error.message.is_empty(), "{body}");
    }
}

#[test]
fn a_value_that_is_not_sealed_is_refused() {
    // The opener is strict: a caller above the envelope decides what a raw
    // value means, so open can never pass plaintext through by accident.
    for value in ["sk-legacy-plaintext", "postgres://u:p@h/db"] {
        let error = open(value).expect_err("a raw value must not open");
        assert_eq!(error.code, ErrorCode::ConfigCorrupt);
        assert!(
            !error.message.contains(value),
            "the message echoes the value: {}",
            error.message
        );
    }
}

#[test]
fn is_sealed_reads_the_marker_and_nothing_else() {
    assert!(is_sealed(SEALED_PREFIX));
    assert!(is_sealed(&seal_with("sk-marker-check")));
    assert!(!is_sealed(""));
    assert!(!is_sealed("sk-marker-check"));
    assert!(!is_sealed("Sealed:v1:AAAA"), "the marker is case sensitive");
    assert!(!is_sealed(" sealed:v1:AAAA"), "no leading space counts");
}

#[test]
fn the_sealed_path_list_is_explicit_and_covers_both_targets() {
    // The plan's two targets.
    assert!(is_sealed_path("store.database_url"));
    assert!(is_sealed_path("credentials.api_key"));
    assert!(is_sealed_path("credentials.embed_api_key"));
    assert!(is_sealed_path("credentials.rerank_api_key"));
    assert!(is_sealed_path("credentials.extract_api_key"));
    assert!(is_sealed_path("CREDENTIALS.API_KEY"), "case insensitive");
    // A future credential under the same parent is sealed without a new edit.
    assert!(is_sealed_path("credentials.anything_new"));

    // Everything a user can set that is not a secret.
    for path in [
        "log.level",
        "injection.budget_tokens",
        "history.lesson_max_tokens",
        "history.similarity_threshold",
        "history.default_limit",
        "history.max_limit",
        "backend",
        "bank.fixed",
        "bank.policy",
        "sync.omit",
        "store.sqlite_path",
        "extract.provider",
        "extract.url",
        "extract.model",
        "extract.temperature",
        "embed.url",
        "embed.model",
        "embed.dims",
        "embed.provider",
        "embed.switch_space",
        "rerank.url",
        "rerank.model",
        "rerank.provider",
    ] {
        assert!(!is_sealed_path(path), "{path} must stay readable");
    }

    // The object itself is not a leaf, so it is not a sealed path.
    assert!(!is_sealed_path("credentials"));
}

#[test]
fn the_authenticated_context_is_a_constant() {
    // Binding the config path instead would make a moved secret permanently
    // unreadable, so the context carries no path at all.
    assert_eq!(AAD, b"clio-secret-seal-v1");
}

#[test]
fn the_process_key_is_used_when_no_explicit_key_is_given() {
    use super::test_key;
    test_key::use_key(KEY);
    let sealed = seal("sk-process-key-check").expect("seal with the process key");
    assert_eq!(
        open(&sealed),
        Ok("sk-process-key-check".to_owned()),
        "the same process key seals and opens"
    );
    test_key::clear_thread_key();
}

fn seal_with(value: &str) -> String {
    super::seal_with_key(value, &KEY).expect("seal")
}

fn seal_under(key: &[u8; KEY_LEN], value: &str) -> String {
    super::seal_with_key(value, key).expect("seal")
}

fn open_with(stored: &str) -> Result<String, clio_types::AmError> {
    super::open_with_key(stored, &KEY)
}
