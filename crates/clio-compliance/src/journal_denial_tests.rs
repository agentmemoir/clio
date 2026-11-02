// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the production erasure-denial seam: a fresh deployment is
//! clear, a recorded denial survives and keeps its generation, a damaged
//! journal is unavailable and refuses a write, a journal recorded under another
//! salt is unavailable for every subject rather than clear, a probe that
//! resolved no deployment salt fails closed, a journal this deployment cannot
//! name is refused rather than probed, and the seam is usable as a trait object.
//!
//! ## Owns
//! - The scratch journal fixture and the seam assertions
//!
//! ## Does not own
//! - The seam itself (`journal_denial`) or the journal under it
//!   (`erasure_journal`, `erase`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::path::{Path, PathBuf};

use clio_store::{DenialVerdict, ErasureDenial, sha256_hex};
use clio_types::ErrorCode;

use super::JournalDenialProbe;
use crate::salt::{tombstone_salt_at, tombstone_salt_fingerprint_of};
use crate::test_unique::unique;

/// A unique scratch directory removed on drop (the `erasure_journal_tests`
/// fixture shape).
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(unique("clio-journal-denial"));
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

/// Seed raw bytes and restrict them to the owner where mode bits exist.
fn write_raw(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).expect("seed the raw journal");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("restrict the seeded journal");
    }
}

/// A missing journal is clear — a fresh deployment must work — and a recorded
/// denial becomes the verdict without double-applying on a repeat.
#[test]
fn a_fresh_deployment_is_clear_and_a_recorded_denial_keeps_its_generation() {
    let scratch = Scratch::new();
    let probe = JournalDenialProbe::new(scratch.journal());
    assert_eq!(probe.verdict("subject-a"), DenialVerdict::Clear);
    assert_eq!(
        probe
            .record_denial("subject-a", &["bank-b".to_owned(), "bank-a".to_owned()])
            .expect("record the denial"),
        1,
        "a first denial starts generation 1"
    );
    assert_eq!(probe.verdict("subject-a"), DenialVerdict::Denied);
    assert_eq!(
        probe.verdict("subject-b"),
        DenialVerdict::Clear,
        "another subject is untouched"
    );
    assert_eq!(
        probe
            .record_denial("subject-a", &["bank-a".to_owned()])
            .expect("repeat the denial"),
        1,
        "a redelivered denial cannot double-apply"
    );
    // The write landed in the existing journal format, under this deployment's
    // salt, and the fingerprint names that salt.
    let journal = match crate::erasure_journal::load_journal(&scratch.journal()) {
        crate::erasure_journal::JournalState::Current(journal) => journal,
        other => panic!("expected a current journal: {other:?}"),
    };
    assert_eq!(journal.generation, 1);
    assert_eq!(journal.denials.len(), 1);
    assert_eq!(
        journal.salt_fingerprint,
        Some(probe.salt_fingerprint()),
        "the journal records the salt the probe answers under"
    );
}

/// A journal written through the erasure path's own helpers answers through
/// the probe: the seam reuses the stored identity, not a parallel one.
#[test]
fn the_probe_reads_the_identity_the_erasure_path_writes() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let salt = tombstone_salt_at(&path).expect("resolve the deployment salt");
    let hash = sha256_hex(&salt, "subject-x");
    crate::erasure_journal::record_denial(&path, &hash, &["bank-a".to_owned()])
        .expect("record through the journal operation");
    let probe = JournalDenialProbe::new(path);
    assert_eq!(probe.verdict("subject-x"), DenialVerdict::Denied);
}

/// A damaged journal is distinguishable from a clear one and refuses a write
/// without being overwritten.
#[test]
fn a_damaged_journal_is_unavailable_and_a_write_refuses() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let bytes = b"not a journal".to_vec();
    write_raw(&path, &bytes);
    let probe = JournalDenialProbe::new(path.clone());
    assert_eq!(
        probe.verdict("subject-a"),
        DenialVerdict::AuthorityUnavailable
    );
    assert_ne!(
        probe.verdict("subject-a"),
        DenialVerdict::Clear,
        "unreadable is never reported as clear"
    );
    let err = probe
        .record_denial("subject-a", &["bank-a".to_owned()])
        .expect_err("a damaged authority must refuse the write");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert_eq!(
        std::fs::read(&path).expect("reread the damaged journal"),
        bytes,
        "the damaged journal was not overwritten"
    );
}

/// `path` with production's salt suffix pushed on.
fn salt_sidecar(path: &Path) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(".salt");
    PathBuf::from(sidecar)
}

/// A journal body at generation 1 denying `subject_hash` under `fingerprint`.
fn journal_body(fingerprint: &str, subject_hash: &str) -> Vec<u8> {
    format!(
        r#"{{"schema_version":1,"deployment_id":"seeded-deployment","generation":1,"salt_fingerprint":"{fingerprint}","denials":[{{"subject_hash":"{subject_hash}","banks":[]}}]}}"#
    )
    .into_bytes()
}

/// A journal recorded under the salt this deployment resolves answers as it
/// always did: the subject it denies is denied, another subject is clear, and a
/// new denial lands.
#[test]
fn a_journal_recorded_under_the_resolved_salt_answers_as_before() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    write_raw(&salt_sidecar(&path), &[b'a'; 32]);
    let salt = tombstone_salt_at(&path).expect("resolve the deployment salt");
    let body = journal_body(
        &tombstone_salt_fingerprint_of(&salt),
        &sha256_hex(&salt, "subject-a"),
    );
    write_raw(&path, &body);
    let probe = JournalDenialProbe::new(path);
    assert_eq!(probe.verdict("subject-a"), DenialVerdict::Denied);
    assert_eq!(probe.verdict("subject-b"), DenialVerdict::Clear);
    assert_eq!(
        probe
            .record_denial("subject-b", &["bank-a".to_owned()])
            .expect("record the denial"),
        2,
        "the denial lands on the journal recorded under this salt"
    );
}

/// A journal recorded under another salt makes the probe unavailable rather
/// than clear: its denials were hashed under a salt this process does not hold,
/// so no match here is evidence that a subject was never erased. A write
/// refuses too, and the journal is left byte-identical.
#[test]
fn a_journal_recorded_under_another_salt_is_unavailable() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    // This deployment resolves salt A from the sidecar beside the journal.
    write_raw(&salt_sidecar(&path), &[b'a'; 32]);
    // The journal was written under salt B: it denies B's hash of a subject and
    // records B's fingerprint.
    let recorded = [b'b'; 32];
    let body = journal_body(
        &tombstone_salt_fingerprint_of(&recorded),
        &sha256_hex(&recorded, "subject-a"),
    );
    write_raw(&path, &body);
    let probe = JournalDenialProbe::new(path.clone());

    assert_eq!(
        probe.verdict("subject-a"),
        DenialVerdict::AuthorityUnavailable,
        "a subject the other salt recorded as denied is neither denied nor clear here"
    );
    assert_eq!(
        probe.verdict("subject-unrecorded"),
        DenialVerdict::AuthorityUnavailable,
        "a subject this salt cannot find in the journal is never reported clear"
    );
    let err = probe
        .record_denial("subject-unrecorded", &["bank-a".to_owned()])
        .expect_err("a journal under another salt refuses a denial");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert!(
        err.message.contains("different deployment salt"),
        "the refusal names the salt-fingerprint divergence: {}",
        err.message
    );
    assert_eq!(
        std::fs::read(&path).expect("read back"),
        body,
        "the journal recorded under the other salt is byte-identical"
    );
}

/// A journal written before the fingerprint field existed records no salt to
/// verify against, so the probe answers as unavailable rather than clear: an
/// unverifiable journal is never read as one this deployment's salt answers
/// for, even though its denials parse and its shape is current.
#[test]
fn a_journal_with_no_recorded_fingerprint_is_unavailable() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let body = format!(
        r#"{{"schema_version":1,"deployment_id":"legacy-deployment","generation":1,"denials":[{{"subject_hash":"{}","banks":[]}}]}}"#,
        "0".repeat(64)
    );
    write_raw(&path, body.as_bytes());
    let probe = JournalDenialProbe::new(path);
    assert_eq!(
        probe.verdict("subject-a"),
        DenialVerdict::AuthorityUnavailable,
        "a journal with no recorded fingerprint cannot answer for this salt"
    );
}

/// A probe that resolved no deployment salt fails closed on every answer: no
/// subject reads as clear, a write refuses with the code and the missing salt
/// named, and the fingerprint is blank rather than fabricated, so a caller that
/// refuses a blank fingerprint never records an acknowledgement.
#[test]
fn a_probe_with_no_resolved_salt_fails_closed() {
    if std::env::var("CLIO_COMPLIANCE_SALT").is_ok() {
        // The override supplies the salt as text, so resolution cannot fail
        // here; the deployment in this test would resolve like any other.
        return;
    }
    let scratch = Scratch::new();
    let path = scratch.journal();
    // A sidecar that is not exactly the required length is refused and never
    // regenerated, so construction holds no salt.
    write_raw(&salt_sidecar(&path), b"not-a-salt");
    let probe = JournalDenialProbe::new(path.clone());

    assert_eq!(
        probe.verdict("subject-a"),
        DenialVerdict::AuthorityUnavailable,
        "no salt means no identity to answer about, never clear"
    );
    let err = probe
        .record_denial("subject-a", &["bank-a".to_owned()])
        .expect_err("a probe with no salt must refuse the write");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(
        err.message.contains("deployment salt"),
        "the refusal names the missing salt: {}",
        err.message
    );
    assert_eq!(
        probe.salt_fingerprint(),
        String::new(),
        "the fingerprint is blank rather than invented"
    );
    assert!(
        !path.exists(),
        "no journal was written by a probe that could not hash"
    );
}

/// A deployment that cannot name its journal is refused rather than probed:
/// there is no fallback, because a route serving without the authority would
/// ignore every recorded denial.
#[test]
fn an_unresolvable_journal_path_refuses_to_build_a_probe() {
    let unnamed = |_: &str| -> Option<String> { None };
    let error = JournalDenialProbe::resolved(None, &unnamed)
        .err()
        .expect("nowhere for the journal to live");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    let message = error.message.as_str();
    assert!(
        message.contains(clio_config::ERASURE_JOURNAL_FILE_ENV),
        "the refusal names the override that fixes it: {message}"
    );
}

/// The probe reads the same journal whether a host names it in an effective
/// document or leaves it to the deployment resolution.
#[test]
fn a_document_path_this_deployment_resolves_is_the_journals_path() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let get = |key: &str| {
        (key == clio_config::ERASURE_JOURNAL_FILE_ENV).then(|| path.display().to_string())
    };
    for document in [None, Some(path.as_path())] {
        assert_eq!(
            JournalDenialProbe::resolved(document, &get)
                .expect("the resolved journal")
                .journal_path(),
            path,
            "document {document:?}"
        );
    }
}

/// The probe serves the seam as a trait object, naming its journal path.
#[test]
fn the_seam_answers_through_a_trait_object() {
    let scratch = Scratch::new();
    let probe = JournalDenialProbe::new(scratch.journal());
    let seam: &dyn ErasureDenial = &probe;
    assert_eq!(seam.verdict("subject-a"), DenialVerdict::Clear);
    assert_eq!(seam.salt_fingerprint(), probe.salt_fingerprint());
    assert_eq!(
        probe.journal_path(),
        scratch.root.join("erasure-journal.json")
    );
}
