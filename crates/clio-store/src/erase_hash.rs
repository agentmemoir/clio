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
    fn digest_is_deterministic_and_salt_sensitive() {
        let a = sha256_hex(b"salt", "subject-1");
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(a, sha256_hex(b"salt", "subject-1"));
        assert_ne!(a, sha256_hex(b"other-salt", "subject-1"));
        assert_ne!(a, sha256_hex(b"salt", "subject-2"));
        // Known SHA-256 of `b"salt\x1fsubject-1"`.
        assert_eq!(
            a,
            "2004fa2bb58bc9058f2f09518b1127c5f1efac30cab7422b91120d6874b720a8"
        );
    }
}
