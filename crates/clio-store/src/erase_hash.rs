// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one-way hashing of data-subject and item IDs
//! written to the content-free compliance tombstone ledger.
//!
//! ## Owns
//! - Salted SHA-256 hex encoding of subject/item identifiers
//!
//! ## Does not own
//! - Tombstone persistence or erasure orchestration
//! - Salt sourcing / rotation policy (caller supplies the deployment salt)
//!
//! ## Boundary
//!
//! Changes to the tombstone hash construction should normally require changes here.
//! Changes to when tombstones are written should not require changes here.
//!
//! Keep this module focused on the irreversible identifier hash.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// Domain-separation byte between the deployment salt and the identifier.
const SEP: u8 = 0x1f;

/// The use a digest serves, one variant per call site.
///
/// The variant is the domain tag: two uses of the same value produce different
/// digests, and a digest from one use never matches another. The tag is not
/// secret. It separates uses; it does not key them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// Belief context change-detection telemetry.
    BeliefContext,
    /// Item context change-detection telemetry.
    ItemContext,
    /// Persona context change-detection telemetry.
    PersonaContext,
    /// Plaintext-mode sync content digest.
    PlaintextDigest,
    /// Plaintext-mode sync mutation idempotency key.
    PlaintextMutationId,
    /// Sync cursor checksum.
    SyncCursor,
    /// Sync watermark checksum.
    SyncWatermark,
}

impl Domain {
    /// The four tag bytes this domain hashes under.
    fn tag(self) -> [u8; 4] {
        (self as u32).to_be_bytes()
    }
}

/// One-way SHA-256 hex digest of `tag || 0x1f || value` for `domain`.
///
/// Callers name the use, never the tag bytes. The result is a 64-character
/// lowercase hex string.
#[must_use]
pub fn domain_sha256_hex(domain: Domain, value: &str) -> String {
    sha256_hex(&domain.tag(), value)
}

/// One-way SHA-256 hex digest of `salt || 0x1f || value`.
///
/// The result is a 64-character lowercase hex string; the input identifier is
/// never recoverable from it.
#[must_use]
pub fn sha256_hex(salt: &[u8], value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update([SEP]);
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in &digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_deterministic_and_domain_sensitive() {
        let a = domain_sha256_hex(Domain::BeliefContext, "subject-1");
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(a, domain_sha256_hex(Domain::BeliefContext, "subject-1"));
        assert_ne!(a, domain_sha256_hex(Domain::ItemContext, "subject-1"));
        assert_ne!(a, domain_sha256_hex(Domain::BeliefContext, "subject-2"));
    }

    #[test]
    fn low_level_digest_hashes_tag_then_separator_then_value() {
        // Known SHA-256 of the `BeliefContext` tag bytes, `0x1f`, then the value.
        assert_eq!(
            sha256_hex(&Domain::BeliefContext.tag(), "subject-1"),
            "5b4b22e4cb21840e580bde89f4dc32ab0531ece014b8e0d17d9d2f394777b9b9"
        );
    }

    #[test]
    fn every_domain_has_a_distinct_tag() {
        let domains = [
            Domain::BeliefContext,
            Domain::ItemContext,
            Domain::PersonaContext,
            Domain::PlaintextDigest,
            Domain::PlaintextMutationId,
            Domain::SyncCursor,
            Domain::SyncWatermark,
        ];
        for (i, a) in domains.iter().enumerate() {
            for b in &domains[i + 1..] {
                assert_ne!(a.tag(), b.tag());
            }
        }
    }
}
