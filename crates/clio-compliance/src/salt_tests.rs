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

use super::{resolve_salt, tombstone_salt_fingerprint_at, tombstone_salt_fingerprint_of};
use crate::test_unique::unique;
use clio_types::ErrorCode;
use std::path::{Path, PathBuf};

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
    set_mode(path, 0o600);
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
    let second = Scratch::new();
    let other = resolve_salt(None, &second.journal()).expect("a second deployment's salt");
    assert_ne!(
        salt, other,
        "each deployment mints its own salt, so no shared constant keys two deployments"
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
        tombstone_salt_fingerprint_of(&recorded),
        "the fingerprint describes the recorded salt"
    );
    let other = tombstone_salt_fingerprint_of(
        &resolve_salt(Some("another-deployment"), &journal).expect("a configured salt resolves"),
    );
    assert_ne!(
        fingerprint, other,
        "a different salt yields a different fingerprint"
    );
    let second = Scratch::new();
    assert_ne!(
        fingerprint,
        tombstone_salt_fingerprint_at(&second.journal())
            .expect("a second deployment's fingerprint"),
        "the fingerprint is keyed by each deployment's own salt, not a shared constant"
    );
}

#[test]
fn a_salt_with_whitespace_at_an_end_reads_back_unchanged() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let mut seeded = [b'a'; 32];
    seeded[0] = b' ';
    seeded[31] = b'\n';
    seed_owner_only(&sidecar(&journal), &seeded);
    let salt = resolve_salt(None, &journal).expect("an exact-length salt is read as written");
    assert_eq!(
        salt,
        seeded.to_vec(),
        "a generated salt may begin or end with a whitespace byte and must still be reused"
    );
}

/// A trailing newline is tolerated: a salt a provisioner wrote with `echo` is
/// thirty-three bytes on disk and still resolves, and the bytes kept are the
/// thirty-two of the salt rather than the thirty-three of the file.
#[test]
fn a_trailing_newline_on_a_full_length_salt_is_tolerated() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let mut seeded = [b'a'; 33];
    seeded[32] = b'\n';
    seed_owner_only(&sidecar(&journal), &seeded);
    let salt = resolve_salt(None, &journal).expect("a trailing newline is trimmed away");
    assert_eq!(
        salt,
        vec![b'a'; 32],
        "the newline is dropped, so the salt is the thirty-two bytes it surrounds"
    );
}

/// A sidecar the read cannot open is a refusal rather than a regeneration:
/// regenerating would have to replace whatever is there, which is exactly what
/// the length rule exists to prevent.
#[test]
fn a_sidecar_that_cannot_be_read_is_refused() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let path = sidecar(&journal);
    std::fs::create_dir(&path).expect("a directory stands where the salt file belongs");
    let err = resolve_salt(None, &journal).expect_err("an unreadable sidecar is refused");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(
        err.message.contains("failed to read"),
        "the refusal names the operation that failed: {}",
        err.message
    );
    assert!(
        path.is_dir(),
        "the path that could not be read is left exactly as it was found"
    );
}

/// `create_new` is one `O_CREAT|O_EXCL`, so a path that already exists is never
/// written through: here a planted link with no target stands where the sidecar
/// belongs. The would-be winner's file cannot be read back, so this call
/// refuses rather than writing through the link or minting a second salt.
#[cfg(unix)]
#[test]
fn a_provisioned_but_unreadable_sidecar_is_refused() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let path = sidecar(&journal);
    let target = scratch.root.join("planted-target");
    std::os::unix::fs::symlink(&target, &path).expect("plant a link at the salt path");
    let err =
        resolve_salt(None, &journal).expect_err("a provisioned but unreadable sidecar is refused");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(
        err.message.contains("provisioned concurrently"),
        "the refusal names the race it lost: {}",
        err.message
    );
    assert!(
        !target.exists(),
        "nothing is written through the planted link at {}",
        path.display()
    );
}

/// A directory that denies writes is a real refusal of its own: the sidecar is
/// absent, so the read finds nothing and the create is what fails.
///
/// Root, and any process holding `CAP_DAC_OVERRIDE`, writes into the directory
/// whatever its mode, so the test asks the process's own access which case this
/// is and asserts what that case allows.
#[cfg(unix)]
#[test]
fn a_directory_that_denies_writes_yields_a_create_refusal() {
    let scratch = Scratch::new();
    let locked = scratch.root.join("locked");
    std::fs::create_dir(&locked).expect("create the locked directory");
    set_mode(&locked, 0o500);
    let refused = write_is_refused(&locked);
    let outcome = resolve_salt(None, &locked.join("erasure-journal.json"));
    set_mode(&locked, 0o700);
    if refused {
        let err = outcome.expect_err("a read-only directory takes no salt file");
        assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
        assert!(
            err.message.contains("failed to create"),
            "the refusal names the create it could not do: {}",
            err.message
        );
        assert!(
            !sidecar(&locked.join("erasure-journal.json")).exists(),
            "no salt file appears where the create was refused"
        );
    } else {
        let salt = outcome.expect("a DAC-bypassing process writes into the directory");
        assert_eq!(salt.len(), 32, "the salt it wrote is a generated salt");
    }
}

/// Restrict `path` to `mode`, so a length or refusal assertion is not pre-empted
/// by whatever the platform's default mode happens to be.
#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .unwrap_or_else(|err| panic!("set mode {mode:o} on {}: {err}", path.display()));
}

/// Whether this process is actually refused a write into `dir`.
///
/// Root, and any process holding `CAP_DAC_OVERRIDE`, writes into a directory
/// whatever its mode, so a test that asserts a write refusal cannot observe one
/// there. The probe asks the question directly instead of reading the uid, and
/// it removes the file it created when the write is allowed.
#[cfg(unix)]
fn write_is_refused(dir: &Path) -> bool {
    let probe = dir.join(".write-probe");
    match std::fs::write(&probe, b"x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            false
        }
        Err(_) => true,
    }
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
