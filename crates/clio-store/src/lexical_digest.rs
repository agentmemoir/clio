// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for turning assembled item text and query text
//! into the keyed digests the lexical index stores and matches, so the stored
//! column carries no readable word.
//!
//! ## Owns
//! - Token normalization (case folding, accent folding, alphanumeric runs)
//! - The stored and query encodings (space-joined `k`-prefixed hex digests)
//!
//! ## Does not own
//! - The keyed-digest construction itself, which this index shares with every
//!   other keyed digest in the workspace (`keyed_digest`)
//! - The stemming algorithm itself (`lexical_porter`)
//! - Where the index key comes from (`dek`)
//! - Per-backend SQL and tokenizer settings (`sqlite_index` / `postgres_index`)
//!
//! ## Boundary
//!
//! Changes to the digest construction or the stored encoding should normally
//! require changes here. Changes to the SQL that stores or matches the encoded
//! text should not require changes here.
//!
//! The same transformation runs on the write path and the query path, so a
//! query term and the stored form of its base word always meet. The stored form
//! is a keyed digest, never an encoding: without the key the column is opaque.
//!
//! Keep this module focused on the one keyed term encoding.

use crate::keyed_digest::keyed_digest_hex;
use crate::lexical_porter::stem;
use std::fmt::Write as _;

/// Bytes of the HMAC output kept per term: 128 bits, far beyond the collision
/// budget of any store, and half the hex of a full SHA-256.
const DIGEST_BYTES: usize = 16;

/// Hex length of one encoded term digest.
pub(crate) const DIGEST_HEX_LEN: usize = DIGEST_BYTES * 2;

/// Lowercase-letter prefix that every stored term carries.
///
/// Hex digests can begin with digits, and a PostgreSQL token beginning with
/// digits can parse as a number: a term like `002e5aa3…` is split by the text
/// search parser into an `sfloat` and a `numword` (measured on 20 000 random
/// 32-char hex digests: 6.6% split). One letter in front makes every term a
/// single word token on both backends, and keeps the stored form uniform.
const DIGEST_PREFIX: char = 'k';

/// Length of one stored term, prefix included.
pub(crate) const DIGEST_TERM_LEN: usize = DIGEST_HEX_LEN + 1;

/// Domain separation, so the index key can never be confused with a content key
/// and a future encoding change can be published as a new version. The label
/// is part of the message the shared keyed-digest construction hashes.
const DOMAIN: &[u8] = b"clio-lexical-blind-index-v1\0";

/// Encode assembled item text or query text as space-joined keyed digests.
///
/// Every alphanumeric run is lowercased, accent-folded, and stemmed, then
/// replaced by one `k`-prefixed 32-hex-char HMAC-SHA256 truncation under `key`.
/// Text with no alphanumeric run encodes to an empty string, which callers read
/// as "no terms" rather than as a match-everything pattern.
#[must_use]
pub(crate) fn keyed_terms(key: &[u8; 32], text: &str) -> String {
    let mut encoded = String::new();
    for token in tokens(text) {
        if !encoded.is_empty() {
            encoded.push(' ');
        }
        let _ = write!(encoded, "{}", keyed_digest_term(key, &stem(&token)));
    }
    encoded
}

/// Split `text` into normalized tokens, preserving multiplicity and order.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for raw in text.chars() {
        for lower in raw.to_lowercase() {
            let ch = fold_accent(lower);
            if ch.is_alphanumeric() {
                current.push(ch);
            } else if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Fold the common Latin-1 accented letters onto their ASCII base, matching the
/// diacritic removal the database tokenizers apply before indexing.
fn fold_accent(ch: char) -> char {
    match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        _ => ch,
    }
}

/// One stored term: `k` plus the truncated HMAC-SHA256 of the stemmed token,
/// lowercase hex. The prefix keeps the term a single word token under every
/// text search parser the index runs on.
///
/// The digest itself comes from the shared keyed-digest construction, which
/// takes the same domain label and the same width this index has always used,
/// so a stored term is unchanged by that sharing.
fn keyed_digest_term(key: &[u8; 32], token: &str) -> String {
    let mut term = String::with_capacity(DIGEST_TERM_LEN);
    term.push(DIGEST_PREFIX);
    term.push_str(&keyed_digest_hex(
        key,
        DOMAIN,
        token.as_bytes(),
        DIGEST_BYTES,
    ));
    term
}

#[cfg(test)]
#[path = "lexical_digest_tests.rs"]
mod tests;
