// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared readable-export test identity: one
//! age key pair per test process, and the read helper that decrypts a sealed
//! bundle.
//!
//! ## Owns
//! - `test_identity`, `test_recipient`, and `read_sealed`
//!
//! ## Does not own
//! - Export fixtures and the bundle copy helper (see `export_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::path::Path;
use std::sync::OnceLock;

/// One age identity for this test process; every readable test export seals to it.
pub(crate) fn test_identity() -> &'static age::x25519::Identity {
    static IDENTITY: OnceLock<age::x25519::Identity> = OnceLock::new();
    IDENTITY.get_or_init(age::x25519::Identity::generate)
}

/// The age recipient text matching [`test_identity`].
pub(crate) fn test_recipient() -> String {
    test_identity().to_public().to_string()
}

/// Read an exported file as text, decrypting an age-sealed readable bundle.
pub(crate) fn read_sealed(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read the exported file");
    let bytes = if bytes.starts_with(b"age-encryption.org/v1\n") {
        age::decrypt(test_identity(), &bytes).expect("decrypt the sealed export")
    } else {
        bytes
    };
    String::from_utf8(bytes).expect("export text is UTF-8")
}
