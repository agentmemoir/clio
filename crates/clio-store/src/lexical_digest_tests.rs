// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for pinning the keyed lexical term encoding:
//! determinism, key sensitivity, stem parity between write and query text, and
//! the absence of any bare (unkeyed) hash in the stored form.
//!
//! ## Owns
//! - The unit vectors for `lexical_digest`
//!
//! ## Does not own
//! - The encoding itself (`lexical_digest`)
//! - Stemming vectors (`lexical_porter_tests`)
//!
//! ## Boundary
//!
//! Changes to the stored encoding should normally fail a vector here first.
//!
//! Keep this module focused on the encoding's contract.

use super::{DIGEST_HEX_LEN, DIGEST_TERM_LEN, keyed_terms};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// One fixed test key; the real key comes from the key store.
const KEY: [u8; 32] = [0x11; 32];
/// A second key, to observe key sensitivity.
const OTHER_KEY: [u8; 32] = [0x22; 32];

/// Split an encoding into its individual digest tokens.
fn terms(encoded: &str) -> Vec<&str> {
    encoded.split_whitespace().collect()
}

/// Lowercase hex of `bytes`, the shape an unkeyed attacker would build.
fn hex_of(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The stored encoding is pinned to fixed values. The construction was shared
/// with the sync key roles, and a share that moved any of these bytes would
/// silently rewrite every stored term, so these are the vectors that catch it.
#[test]
fn the_stored_encoding_matches_its_pinned_vectors() {
    assert_eq!(
        keyed_terms(&KEY, "alpha beta gamma"),
        "ka0e0ed43e744db1b68cc32b8fac47b29 k1753997562072e23fc276655501b9653 \
         kbb013e649ec606fd050e64f35b3bb1ee",
        "three pinned terms under the fixed key"
    );
    assert_eq!(
        keyed_terms(&KEY, "zephyr"),
        "kdecb1420729be8a356cdb3d5e2bf2606",
        "one pinned term under the fixed key"
    );
}

#[test]
fn deterministic_and_key_sensitive() {
    let a = keyed_terms(&KEY, "Deploying the zephyr service");
    let b = keyed_terms(&KEY, "Deploying the zephyr service");
    let c = keyed_terms(&OTHER_KEY, "Deploying the zephyr service");
    assert_eq!(a, b, "same key and text must encode identically");
    assert_ne!(a, c, "a different key must encode differently");
    assert_ne!(a, keyed_terms(&KEY, "Deploying the zephyr vault"));
}

#[test]
fn every_term_is_a_parse_stable_prefixed_hex_digest() {
    let encoded = keyed_terms(&KEY, "alpha beta gamma");
    let alpha_terms = terms(&encoded);
    assert_eq!(alpha_terms.len(), 3);
    for term in alpha_terms {
        assert_eq!(term.len(), DIGEST_TERM_LEN);
        // A leading letter is what keeps the token one word under both text
        // search parsers; hex alone can parse as a number (see `lexical_digest`).
        assert_eq!(&term[..1], "k", "term must carry the letter prefix: {term}");
        assert!(
            term[1..].chars().all(|c| c.is_ascii_hexdigit()),
            "the body after the prefix is hex: {term}"
        );
    }
    // Order and multiplicity are preserved: the encoding is positional.
    let repeated = keyed_terms(&KEY, "alpha alpha");
    let repeated_terms = terms(&repeated);
    assert_eq!(repeated_terms.len(), 2);
    assert_eq!(repeated_terms[0], repeated_terms[1]);
}

#[test]
fn stored_form_is_not_a_bare_hash_or_encoding() {
    let secret = "zephyr";
    let encoded = keyed_terms(&KEY, secret);
    let stored = terms(&encoded)[0];
    // A bare SHA-256 of the word, in the shape a dictionary attack would build.
    let bare = Sha256::digest(secret.as_bytes());
    let bare_hex = hex_of(&bare);
    assert_ne!(
        stored, bare_hex,
        "a bare hash would be dictionary-reversible"
    );
    let hex_of_word = hex_of(secret.as_bytes());
    assert_ne!(stored, hex_of_word, "the word must not be hex-encoded");
    assert!(
        !stored.contains(secret),
        "the word must not survive in the digest"
    );
    // The unkeyed digest appears nowhere in the stored form.
    assert!(!encoded.contains(&bare_hex[..DIGEST_HEX_LEN]));
}

#[test]
fn inflection_matches_because_both_sides_stem() {
    // Written text and a query inflect differently; both sides stem to `deploy`.
    let document = keyed_terms(&KEY, "Deploying the zephyr service failed");
    let query = keyed_terms(&KEY, "deployed");
    let query_terms = terms(&query);
    let query_term = query_terms[0];
    assert!(
        terms(&document).contains(&query_term),
        "an inflected query must meet the base form stored at index time"
    );
}

#[test]
fn case_and_accent_fold_together() {
    assert_eq!(
        keyed_terms(&KEY, "Café ECONNRESET"),
        keyed_terms(&KEY, "cafe econnreset")
    );
    assert_eq!(keyed_terms(&KEY, "ŚWIAT"), keyed_terms(&KEY, "świat"));
}

#[test]
fn empty_and_punctuation_only_input_encodes_to_nothing() {
    assert_eq!(keyed_terms(&KEY, ""), "");
    assert_eq!(keyed_terms(&KEY, "   "), "");
    assert_eq!(keyed_terms(&KEY, "*** --- ..."), "");
    // Digits survive as terms; they are not digits-only content filters.
    assert_eq!(terms(&keyed_terms(&KEY, "8080")).len(), 1);
}

#[test]
fn longer_text_is_a_space_joined_expansion() {
    let encoded = keyed_terms(&KEY, "vault compaction");
    let expected = format!(
        "{} {}",
        keyed_terms(&KEY, "vault"),
        keyed_terms(&KEY, "compaction")
    );
    assert_eq!(encoded, expected);
}
