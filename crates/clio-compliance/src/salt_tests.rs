// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the deployment salt tests: the override, the
//! first-use generation beside the journal, the persisted file's length and
//! mode, and the fingerprint derived from it.
//!
//! ## Owns
//! - The resolution assertions for every salt source
//!
//! ## Does not own
//! - The identifier hashing the salt keys (`clio-store`)
//! - The journal the salt sits beside
//!
//! ## Boundary
//!
//! Tests here break when the salt's source, its persisted shape, or its
//! fingerprint changes.
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the deployment salt.

use super::{fingerprint_for, resolve_salt, tombstone_salt_fingerprint_at};
use crate::test_unique::unique;
use clio_types::ErrorCode;
use std::path::{Path, PathBuf};

/// The built-in salt this module must never fall back to.
const PUBLISHED: &[u8] = b"clio-compliance-tombstone-v1";

/// A unique scratch directory removed on drop.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(unique("clio-compliance-salt"));
        std::fs::create_dir_all(&root).expect("create the scratch root");
        Self { root }
    }

    fn journal(&self) -> PathBuf {
        self.root.join("erasure-journal.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The owner-only sibling a salt is persisted at for `journal`.
fn sidecar(journal: &Path) -> PathBuf {
    let mut name = journal.as_os_str().to_os_string();
    name.push(".salt");
    PathBuf::from(name)
}

/// Seed a raw file restricted to the owner, so a length assertion is not
/// pre-empted by whatever the platform's default mode happens to be.
fn seed_owner_only(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).expect("seed the raw file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("restrict the seeded file");
    }
}

#[test]
fn a_configured_value_wins_and_creates_no_file() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let salt = resolve_salt(Some("custom"), &journal).expect("a configured value resolves");
    assert_eq!(salt, b"custom", "the configured value is the salt");
    assert!(
        !sidecar(&journal).exists(),
        "a configured value creates no sidecar"
    );
}

#[test]
fn an_unset_value_never_yields_the_published_constant() {
    let scratch = Scratch::new();
    let salt = resolve_salt(None, &scratch.journal()).expect("a first-use salt is generated");
    assert_eq!(salt.len(), 32, "a generated salt is 32 bytes");
    assert_ne!(
        salt.as_slice(),
        PUBLISHED,
        "a deployment with no override is not hashed under the published constant"
    );
}

#[test]
fn the_first_use_salt_is_reused_and_owner_only() {
    let scratch = Scratch::new();
    // A fresh data directory has no journal parent yet, so the first call
    // creates it rather than refusing.
    let journal = scratch.root.join("fresh").join("erasure-journal.json");
    let first = resolve_salt(None, &journal).expect("generate the salt");
    let second = resolve_salt(None, &journal).expect("re-read the salt");
    assert_eq!(first.len(), 32, "the generated salt is 32 bytes");
    assert_eq!(first, second, "the persisted salt is reused");
    let path = sidecar(&journal);
    assert!(
        path.is_file(),
        "the salt is persisted beside the journal at {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the salt file is owner-only");
    }
}

#[test]
fn a_wrong_length_salt_file_is_refused() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let path = sidecar(&journal);
    seed_owner_only(&path, &[b'x'; 16]);
    let err = resolve_salt(None, &journal).expect_err("a 16-byte salt file is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert_eq!(
        std::fs::read(&path).expect("read back"),
        vec![b'x'; 16],
        "the refused file is neither replaced nor regenerated"
    );
}

#[test]
fn the_fingerprint_follows_the_recorded_salt() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let fingerprint = tombstone_salt_fingerprint_at(&journal).expect("fingerprint the salt");
    assert_eq!(fingerprint.len(), 64, "a SHA-256 digest is 64 characters");
    assert!(
        fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "the fingerprint is lowercase hex: {fingerprint}"
    );
    let recorded = resolve_salt(None, &journal).expect("the recorded salt");
    assert_eq!(
        fingerprint,
        fingerprint_for(&recorded),
        "the fingerprint describes the recorded salt"
    );
    let other = fingerprint_for(
        &resolve_salt(Some("another-deployment"), &journal).expect("a configured salt resolves"),
    );
    assert_ne!(
        fingerprint, other,
        "a different salt yields a different fingerprint"
    );
    assert_ne!(
        fingerprint,
        fingerprint_for(PUBLISHED),
        "the fingerprint is not a function of the published constant"
    );
}

#[test]
fn a_blank_configured_value_is_refused() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let err = resolve_salt(Some("   "), &journal).expect_err("a blank override is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert!(
        err.message.contains("CLIO_COMPLIANCE_SALT"),
        "the refusal names the override: {}",
        err.message
    );
    assert!(
        !sidecar(&journal).exists(),
        "a refused override creates no sidecar and falls back to nothing"
    );
}
