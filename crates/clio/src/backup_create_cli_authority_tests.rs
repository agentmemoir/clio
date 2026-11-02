// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the erasure authority `clio backup create`
//! records: the fingerprint the archive names, and the journal the create
//! refuses to label.
//!
//! ## Owns
//! - The `current_authority` assertions over a matching, an other-salt, a
//!   missing, and a damaged journal
//!
//! ## Does not own
//! - The command implementation (`backup_create_cli`) or the journal model and
//!   its salt rule (`clio-compliance`)
//!
//! ## Boundary
//!
//! Tests here break when the fingerprint an archive records, or the rule that
//! refuses a journal recorded under another salt, changes.
//!
//! Test-only. Never compiled into production builds.

use super::current_authority;
use crate::backup_cli_fixtures::Scratch;
use clio_compliance::{DenialRecord, ErasureJournal, SCHEMA_VERSION};
use clio_types::ErrorCode;
use std::path::{Path, PathBuf};

/// The sidecar path production reads the deployment salt from for `journal`.
fn salt_sidecar(journal: &Path) -> PathBuf {
    let mut sidecar = journal.as_os_str().to_os_string();
    sidecar.push(".salt");
    PathBuf::from(sidecar)
}

/// Seed a raw file restricted to the owner, so the owner-only rule the salt and
/// the journal are read under is satisfied where mode bits exist.
fn seed_owner_only(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).expect("seed the raw file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("restrict the seeded file");
    }
}

/// A journal at generation 1 denying `subject_hash` under `fingerprint`.
fn journal_under(fingerprint: Option<&str>, subject_hash: &str) -> ErasureJournal {
    ErasureJournal {
        schema_version: SCHEMA_VERSION,
        deployment_id: "seeded-deployment".to_owned(),
        generation: 1,
        salt_fingerprint: fingerprint.map(str::to_owned),
        denials: vec![DenialRecord {
            subject_hash: subject_hash.to_owned(),
            banks: Vec::new(),
        }],
    }
}

/// Seed `journal` at `path` as the owner-only file the loader reads.
fn seed_journal(path: &Path, journal: &ErasureJournal) {
    let bytes = serde_json::to_vec(journal).expect("encode the seeded journal");
    seed_owner_only(path, &bytes);
}

/// A journal recorded under the salt this deployment resolves is the authority
/// the archive records, and the fingerprint is the one the journal already
/// holds: the create never labels an archive with a salt its own journal was
/// not written under.
#[test]
fn a_matching_journal_records_its_own_fingerprint() {
    let scratch = Scratch::new("authority-matching");
    let journal_path = scratch.join("erasure-journal.json");
    let live = clio_compliance::tombstone_salt_fingerprint_at(&journal_path)
        .expect("resolve the deployment salt");
    seed_journal(&journal_path, &journal_under(Some(&live), "subject-a"));
    let authority = current_authority(&journal_path)
        .expect("a matching journal records an authority")
        .expect("a current journal is an authority");
    assert_eq!(
        authority.salt_fingerprint,
        Some(live),
        "the archive records the journal's own fingerprint"
    );
    assert_eq!(authority.generation, 1, "the generation the journal holds");
    assert_eq!(authority.deployment_id, "seeded-deployment");
}

/// A journal recorded under another salt refuses: recording the live
/// fingerprint would write an archive the restore guard must reject, because
/// that guard compares the manifest against the journal's own record.
#[test]
fn a_journal_under_another_salt_is_refused() {
    let scratch = Scratch::new("authority-divergent");
    let journal_path = scratch.join("erasure-journal.json");
    let recorded = [b'a'; 32];
    seed_owner_only(&salt_sidecar(&journal_path), &recorded);
    let fingerprint = clio_compliance::tombstone_salt_fingerprint_of(&recorded);
    seed_journal(
        &journal_path,
        &journal_under(Some(&fingerprint), "subject-a"),
    );
    // The deployment now resolves a different salt beside the same journal.
    seed_owner_only(&salt_sidecar(&journal_path), &[b'b'; 32]);
    let err = current_authority(&journal_path).expect_err("another salt refuses the create");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert!(
        err.message.contains("different deployment salt"),
        "the refusal names the salt-fingerprint divergence: {}",
        err.message
    );
}

/// A journal with no recorded fingerprint cannot be verified against any salt,
/// so the create refuses it exactly as it refuses a divergent one.
#[test]
fn a_journal_with_no_recorded_fingerprint_is_refused() {
    let scratch = Scratch::new("authority-unrecorded");
    let journal_path = scratch.join("erasure-journal.json");
    seed_journal(&journal_path, &journal_under(None, "subject-a"));
    let err = current_authority(&journal_path).expect_err("an unverifiable journal is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
}

/// A missing or damaged journal records no authority at all, which the create
/// reports as its warning rather than as a failure: there is nothing to label.
#[test]
fn a_missing_or_damaged_journal_records_no_authority() {
    let scratch = Scratch::new("authority-absent");
    let journal_path = scratch.join("erasure-journal.json");
    assert_eq!(
        current_authority(&journal_path).expect("a missing journal records nothing"),
        None,
        "a missing journal is the honest empty authority"
    );
    seed_owner_only(&journal_path, b"not a journal");
    assert_eq!(
        current_authority(&journal_path).expect("a damaged journal records nothing"),
        None,
        "a damaged journal is never labelled with the live fingerprint"
    );
}
