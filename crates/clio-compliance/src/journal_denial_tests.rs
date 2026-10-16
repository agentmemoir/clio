// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the production erasure-denial seam: a fresh deployment is
//! clear, a recorded denial survives and keeps its generation, a damaged
//! journal is unavailable and refuses a write, a journal this deployment cannot
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
use crate::erase::tombstone_salt;
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
    let hash = sha256_hex(&tombstone_salt(), "subject-x");
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
