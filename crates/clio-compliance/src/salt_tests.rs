// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the deployment salt tests: the override, the
//! first-use generation beside the journal, the persisted file's length and
//! mode, the owner-only read rule, and the fingerprint derived from it.
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

use super::{
    create_salt, resolve_salt, tombstone_salt_fingerprint_at, tombstone_salt_fingerprint_of,
};
#[cfg(unix)]
use crate::salt_test_support::set_mode;
use crate::salt_test_support::{Scratch, salt_of, seed_owner_only, sidecar};
use clio_types::ErrorCode;
use std::path::Path;

#[test]
fn a_configured_value_wins_and_creates_no_file() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let salt = resolve_salt(Some("custom-configured-salt-32-bytes!"), &journal)
        .expect("a configured value resolves");
    assert_eq!(
        salt, b"custom-configured-salt-32-bytes!",
        "the configured value is the salt"
    );
    assert_eq!(salt.len(), 32, "the configured value is 32 bytes");
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
        &resolve_salt(Some("another-deployment-salt-32-bytes"), &journal)
            .expect("a configured salt resolves"),
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
    let mut seeded = salt_of(b'a', 32);
    seeded[0] = b' ';
    seeded[31] = b'\n';
    seed_owner_only(&sidecar(&journal), &seeded);
    let salt = resolve_salt(None, &journal).expect("an exact-length salt is read as written");
    assert_eq!(
        salt, seeded,
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
    let mut seeded = salt_of(b'a', 33);
    seeded[32] = b'\n';
    seed_owner_only(&sidecar(&journal), &seeded);
    let salt = resolve_salt(None, &journal).expect("a trailing newline is trimmed away");
    assert_eq!(
        salt,
        seeded[..32].to_vec(),
        "the newline is dropped, so the salt is the thirty-two bytes it surrounds"
    );
}

/// A sidecar the read cannot open is a refusal rather than a regeneration:
/// regenerating would have to replace whatever is there, which is exactly what
/// the owner-only rule exists to prevent.
#[test]
fn a_sidecar_that_cannot_be_read_is_refused() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let path = sidecar(&journal);
    std::fs::create_dir(&path).expect("a directory stands where the salt file belongs");
    let err = resolve_salt(None, &journal).expect_err("an unreadable sidecar is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert!(
        err.message.contains("not a regular file"),
        "the refusal names what it found: {}",
        err.message
    );
    assert!(
        path.is_dir(),
        "the path that could not be read is left exactly as it was found"
    );
}

/// An owner-only sidecar of the exact length is what the deployment salt is,
/// so it is read as written.
#[test]
fn an_owner_only_sidecar_is_read() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let seeded = salt_of(b'k', 32);
    seed_owner_only(&sidecar(&journal), &seeded);
    let salt = resolve_salt(None, &journal).expect("an owner-only sidecar is read");
    assert_eq!(salt, seeded, "the salt is read as written");
}

/// A sidecar that grants any group or other access is refused: any local reader
/// could open it and confirm a guessed subject, which is what a per-deployment
/// salt exists to prevent. The refusal names the mode found and never includes
/// the salt bytes, and the file is left as it was.
#[cfg(unix)]
#[test]
fn a_widened_sidecar_is_refused() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let path = sidecar(&journal);
    let seeded = salt_of(b'a', 32);
    seed_owner_only(&path, &seeded);
    set_mode(&path, 0o644);
    let err = resolve_salt(None, &journal).expect_err("a widened sidecar is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert!(
        err.message.contains("0644"),
        "the refusal names the mode found: {}",
        err.message
    );
    assert!(
        !err.message.contains(&"a".repeat(32)),
        "the refusal carries no salt bytes: {}",
        err.message
    );
    assert_eq!(
        std::fs::read(&path).expect("read back"),
        seeded,
        "the refused sidecar is neither replaced nor regenerated"
    );
}

/// A link at the sidecar name is refused rather than followed, even when it
/// points at a file this build would otherwise read: a link is not the
/// owner-only file the rule is about, and following one would read a salt from
/// outside the path the deployment controls.
#[cfg(unix)]
#[test]
fn a_symlinked_sidecar_is_refused() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let path = sidecar(&journal);
    let target = scratch.root.join("elsewhere.salt");
    let seeded = salt_of(b'a', 32);
    seed_owner_only(&target, &seeded);
    std::os::unix::fs::symlink(&target, &path).expect("plant a link at the salt path");
    let err = resolve_salt(None, &journal).expect_err("a link is never followed");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert!(
        err.message.contains("symbolic link"),
        "the refusal names what it found: {}",
        err.message
    );
    assert!(
        !err.message.contains(&"a".repeat(32)),
        "the refusal carries no salt bytes: {}",
        err.message
    );
    assert!(
        std::fs::symlink_metadata(&path)
            .expect("inspect the planted link")
            .file_type()
            .is_symlink(),
        "the link is left exactly as it was found"
    );
    assert_eq!(
        std::fs::read(&target).expect("read the file the link named"),
        seeded,
        "the file the link named is never written through"
    );
}

/// The create is one `O_CREAT|O_EXCL`, so the loser of a first-use race re-reads
/// the winner's file under the same owner-only and length rules instead of
/// writing through it. The seeded file is exactly the state the loser observes
/// when it loses, so the re-read path is covered without a race.
#[test]
fn a_lost_create_race_reads_the_winners_sidecar() {
    let scratch = Scratch::new();
    let path = sidecar(&scratch.journal());
    let winner = salt_of(b'w', 32);
    seed_owner_only(&path, &winner);
    let salt = create_salt(&path).expect("the loser reads the winner's salt");
    assert_eq!(salt, winner, "the winner's salt is the deployment salt");
    assert_eq!(
        std::fs::read(&path).expect("read back"),
        winner,
        "the winner's file is not written through"
    );
}

/// A winner whose file is not a readable salt is a refusal of its own: the
/// loser reports it rather than minting a second salt beside it.
#[test]
fn a_lost_create_race_refuses_a_winner_file_that_is_not_a_salt() {
    let scratch = Scratch::new();
    let path = sidecar(&scratch.journal());
    seed_owner_only(&path, &[b'w'; 16]);
    let err = create_salt(&path).expect_err("a winner file that is not a salt is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert_eq!(
        std::fs::read(&path).expect("read back"),
        vec![b'w'; 16],
        "the file the race found is left as it was"
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
    let message = err.message.as_str();
    assert!(
        message.contains("CLIO_COMPLIANCE_SALT"),
        "the refusal names the override: {message}"
    );
    assert!(
        message.contains("holds no salt"),
        "the blank rule: {message}"
    );
    assert!(!message.contains("bytes"), "not the length rule: {message}");
    assert!(
        !sidecar(&journal).exists(),
        "a refused override creates no sidecar and falls back to nothing"
    );
}

/// Whitespace is what the blank rule refuses, and that check runs first, so an
/// all-space override of the required thirty-two bytes is still refused as blank.
#[test]
fn a_whitespace_only_override_of_the_required_length_is_refused() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let err = resolve_salt(Some(&" ".repeat(32)), &journal).expect_err("a blank override refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    let message = err.message.as_str();
    assert!(
        message.contains("holds no salt"),
        "the blank rule: {message}"
    );
    assert!(!message.contains("bytes"), "not the length rule: {message}");
    assert!(
        !sidecar(&journal).exists(),
        "a refused override leaves no sidecar"
    );
}

/// A short override is refused rather than used: a salt of a few bytes is
/// guessable, so every subject hash the journal records under it is guessable
/// too. The refusal names the override and the length required, and it never
/// carries the value itself.
#[test]
fn a_short_configured_value_is_refused() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let literal = "~~~~~~~~";
    let err = resolve_salt(Some(literal), &journal).expect_err("an eight-byte override is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    let message = err.message.as_str();
    assert!(
        message.contains("CLIO_COMPLIANCE_SALT"),
        "the refusal names the override: {message}"
    );
    assert!(message.contains("32"), "the minimum: {message}");
    assert!(message.contains("8 bytes"), "the bytes seen: {message}");
    assert!(!message.contains(literal), "the value: {message}");
    assert!(
        !sidecar(&journal).exists(),
        "a refused override creates no sidecar and falls back to nothing"
    );
}

/// A salt longer than thirty-two bytes is accepted unchanged: a deployment that
/// already set one recorded its journal hashes under exactly those bytes, so
/// refusing it would strand every identifier it hashed.
#[test]
fn a_configured_value_longer_than_the_minimum_is_accepted() {
    let scratch = Scratch::new();
    let journal = scratch.journal();
    let configured = "a-configured-salt-of-exactly-forty-bytes";
    let salt = resolve_salt(Some(configured), &journal).expect("a forty-byte override resolves");
    assert_eq!(
        salt,
        configured.as_bytes(),
        "the configured value is taken as written"
    );
}
