// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the durable erasure journal tests: the load
//! classification, the operations, and the failure-closed check.
//!
//! ## Owns
//! - The journal model, operation, and check assertions
//!
//! ## Does not own
//! - The owner-only file mechanics (`erasure_journal_file_tests.rs`)
//! - The operations and check under test
//!
//! ## Boundary
//!
//! Tests here break when the journal's schema, generation, denial, handoff, or check rules change.
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the journal's model, operations, and check.

use super::{
    DenialRecord, ErasureJournal, JournalState, SCHEMA_VERSION, handoff, initialize_new_empty,
    load_journal, record_denial,
};
use crate::test_unique::unique;
use clio_types::ErrorCode;
use std::path::{Path, PathBuf};

/// A unique scratch directory removed on drop.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(unique("clio-compliance-erasure-journal"));
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

/// Load `path`, requiring a current journal.
fn load_current(path: &Path) -> ErasureJournal {
    match load_journal(path) {
        JournalState::Current(journal) => journal,
        other => panic!("expected current at {}: {other:?}", path.display()),
    }
}

/// Record a denial through the slice's public API.
fn record(path: &Path, subject: &str, banks: &[&str]) -> u64 {
    let banks: Vec<String> = banks.iter().map(|bank| (*bank).to_owned()).collect();
    record_denial(path, subject, &banks).expect("record the denial")
}

/// The sidecar path production appends to the journal's own path.
fn salt_sidecar(journal: &Path) -> PathBuf {
    let mut sidecar = journal.as_os_str().to_os_string();
    sidecar.push(".salt");
    PathBuf::from(sidecar)
}

/// The fingerprint that describes `salt`, so a seeded journal names the salt it
/// was written under without consulting the process environment.
fn fingerprint_of(salt: &[u8]) -> String {
    crate::salt::tombstone_salt_fingerprint_of(salt)
}

/// A journal body at generation 1 denying `subject_hash` under `fingerprint`.
fn journal_body(fingerprint: Option<&str>, subject_hash: &str) -> Vec<u8> {
    let journal = ErasureJournal {
        schema_version: SCHEMA_VERSION,
        deployment_id: "seeded-deployment".to_owned(),
        generation: 1,
        salt_fingerprint: fingerprint.map(str::to_owned),
        denials: vec![DenialRecord {
            subject_hash: subject_hash.to_owned(),
            banks: Vec::new(),
        }],
    };
    serde_json::to_vec(&journal).expect("encode the seeded journal")
}

#[test]
fn a_recorded_denial_survives_a_fresh_load() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    assert_eq!(load_journal(&path), JournalState::Missing);
    let generation = record(&path, "subject-a", &["bank-b", "bank-a", "bank-a"]);
    assert_eq!(generation, 1, "the first denial starts generation 1");
    let first = load_current(&path);
    assert_eq!(first.schema_version, SCHEMA_VERSION);
    assert_eq!(first.generation, 1);
    assert_eq!(first.deployment_id.len(), 64);
    assert_eq!(
        first.salt_fingerprint,
        Some(crate::tombstone_salt_fingerprint_at(&path).expect("fingerprint the deployment salt")),
        "the journal records the salt fingerprint it was written under"
    );
    assert_eq!(first.denials.len(), 1, "one denial");
    assert_eq!(first.denials[0].subject_hash, "subject-a");
    let banks = &first.denials[0].banks;
    assert_eq!(banks, &["bank-a", "bank-b"], "sorted, deduplicated");
    assert_eq!(load_current(&path), first, "the restart proof");
    assert!(first.is_denied("subject-a"));
    assert!(!first.is_denied("subject-b"));
}

#[test]
fn damaged_shapes_classify_as_damaged() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let cases: [(&str, &[u8]); 4] = [
        ("truncated", br#"{"schema_version":1,"deployment_id":"d""#),
        (
            "unknown schema version",
            br#"{"schema_version":2,"deployment_id":"d","generation":0,"denials":[]}"#,
        ),
        (
            "unknown field",
            br#"{"schema_version":1,"deployment_id":"d","generation":0,"denials":[],"extra":true}"#,
        ),
        (
            "trailing bytes",
            br#"{"schema_version":1,"deployment_id":"d","generation":0,"denials":[]}trailing"#,
        ),
    ];
    for (name, bytes) in cases {
        write_raw(&path, bytes);
        assert_eq!(load_journal(&path), JournalState::Damaged, "case: {name}");
    }
    let dir = scratch.root.join("occupied");
    std::fs::create_dir(&dir).expect("occupy a path with a directory");
    assert_eq!(load_journal(&dir), JournalState::Damaged, "a directory");
}

#[cfg(unix)]
#[test]
fn a_group_or_other_readable_journal_classifies_as_damaged() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new();
    let path = scratch.journal();
    record(&path, "subject-a", &[]);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("widen the mode");
    assert_eq!(load_journal(&path), JournalState::Damaged);
    let err = record_denial(&path, "subject-b", &[]).expect_err("a widened authority refuses");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
}

#[test]
fn recording_over_a_damaged_journal_refuses_and_leaves_it_untouched() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let damaged = br#"{"schema_version":1,"deployment_id""#;
    write_raw(&path, damaged);
    let err = record_denial(&path, "subject-a", &[]).expect_err("a damaged authority refuses");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    let kept = std::fs::read(&path).expect("read back");
    assert_eq!(kept, damaged, "the damaged authority is never overwritten");
}

#[test]
fn recording_the_same_subject_again_keeps_generation_and_scope() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    assert_eq!(record(&path, "subject-a", &["bank-a"]), 1);
    let before = load_current(&path);
    assert_eq!(record(&path, "subject-a", &["bank-b"]), 1, "no move");
    let after = load_current(&path);
    assert_eq!(after, before, "the recorded scope is never widened");
}

#[test]
fn recording_a_denial_at_an_exhausted_generation_refuses() {
    let scratch = Scratch::new();
    let path = scratch.root.join("erasure-journal.json");
    // The journal names the salt this process resolves, so the exhausted
    // generation is the refusal this test is about.
    let live = crate::tombstone_salt_fingerprint_at(&path).expect("resolve the deployment salt");
    let body = format!(
        r#"{{"schema_version":1,"deployment_id":"d","generation":{},"salt_fingerprint":"{live}","denials":[]}}"#,
        u64::MAX
    );
    write_raw(&path, body.as_bytes());
    let err = record_denial(&path, "subject-a", &[]).expect_err("an exhausted generation");
    assert_eq!(err.code, ErrorCode::OutOfRange, "{}", err.message);
    let kept = std::fs::read(&path).expect("read back");
    assert_eq!(kept, body.as_bytes(), "the saturated journal is untouched");
}

/// A journal recorded under the salt this deployment resolves takes the denial.
#[test]
fn a_journal_recorded_under_the_resolved_salt_accepts_a_denial() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let live = crate::tombstone_salt_fingerprint_at(&path).expect("resolve the deployment salt");
    let body = journal_body(Some(&live), "subject-recorded");
    write_raw(&path, &body);
    assert_eq!(
        record(&path, "subject-new", &["bank-a"]),
        2,
        "the denial lands"
    );
    let loaded = load_current(&path);
    assert!(
        loaded.is_denied("subject-recorded"),
        "the recorded denial is kept"
    );
    assert!(loaded.is_denied("subject-new"));
    assert_eq!(
        loaded.salt_fingerprint,
        Some(live),
        "the fingerprint is unchanged"
    );
}

/// A journal written under another salt cannot take a denial: the hash this
/// process would write is not comparable with the denials already recorded, so
/// the write refuses and leaves the authority byte-identical.
#[test]
fn a_journal_recorded_under_another_salt_refuses_a_denial_and_writes_nothing() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let recorded: Vec<u8> = (0..32u8).map(|i| b'a'.wrapping_add(i)).collect();
    let other: Vec<u8> = (0..32u8).map(|i| b'b'.wrapping_add(i)).collect();
    write_raw(&salt_sidecar(&path), &recorded);
    let body = journal_body(Some(&fingerprint_of(&recorded)), "subject-recorded");
    write_raw(&path, &body);
    // The deployment now resolves a different salt beside the same journal.
    write_raw(&salt_sidecar(&path), &other);
    let err = record_denial(&path, "subject-new", &[]).expect_err("another salt refuses the write");
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

/// A journal with no recorded fingerprint cannot be verified against any salt,
/// so a denial under this deployment's salt refuses rather than adopting it.
#[test]
fn a_journal_with_no_recorded_fingerprint_refuses_a_denial() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let body = journal_body(None, "subject-recorded");
    write_raw(&path, &body);
    let err =
        record_denial(&path, "subject-new", &[]).expect_err("an unverifiable journal refuses");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert!(
        err.message.contains("different deployment salt"),
        "the refusal names the divergence: {}",
        err.message
    );
    assert_eq!(
        std::fs::read(&path).expect("read back"),
        body,
        "the unverifiable journal is never overwritten"
    );
}

#[test]
fn initialize_new_empty_refuses_an_existing_journal() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let created = initialize_new_empty(&path).expect("initialize");
    assert_eq!(created.schema_version, SCHEMA_VERSION);
    assert_eq!(created.generation, 0, "no higher generation");
    assert_eq!(created.denials, Vec::new());
    assert_eq!(created.deployment_id.len(), 64);
    assert_eq!(load_current(&path), created);
    let err = initialize_new_empty(&path).expect_err("an occupied path refuses");
    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert_eq!(load_current(&path), created, "untouched");
}

#[test]
fn initialize_new_empty_refuses_a_damaged_journal() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let damaged = b"not a journal";
    write_raw(&path, damaged);
    let err = initialize_new_empty(&path).expect_err("a damaged path refuses");
    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert_eq!(std::fs::read(&path).expect("read back"), damaged);
}

#[test]
fn handoff_preserves_deployment_generation_and_denials() {
    let scratch = Scratch::new();
    let (source, destination) = (scratch.root.join("src.json"), scratch.root.join("dst.json"));
    initialize_new_empty(&source).expect("initialize the source");
    assert_eq!(record(&source, "subject-a", &["bank-a"]), 1);
    let handed = handoff(&source, &destination).expect("hand off");
    assert_eq!(load_current(&source), handed, "the source is preserved");
    assert_eq!(load_current(&destination), handed, "the copy verifies");
    assert!(handed.is_denied("subject-a"));
    assert_eq!(handed.generation, 1);
}

#[test]
fn handoff_to_the_same_destination_is_idempotent() {
    let scratch = Scratch::new();
    let source = scratch.root.join("source.json");
    let destination = scratch.root.join("destination.json");
    initialize_new_empty(&source).expect("initialize the source");
    let first = handoff(&source, &destination).expect("first handoff");
    let second = handoff(&source, &destination).expect("second handoff");
    assert_eq!(second, first, "the same authority is already there");
}

#[test]
fn handoff_refuses_a_damaged_source() {
    let scratch = Scratch::new();
    let (source, destination) = (scratch.root.join("src.json"), scratch.root.join("dst.json"));
    write_raw(&source, b"{");
    let err = handoff(&source, &destination).expect_err("a damaged source refuses");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert_eq!(load_journal(&destination), JournalState::Missing);
}

#[test]
fn handoff_refuses_a_destination_holding_another_deployment() {
    let scratch = Scratch::new();
    let source = scratch.root.join("source.json");
    let destination = scratch.root.join("destination.json");
    initialize_new_empty(&source).expect("initialize the source");
    let other = initialize_new_empty(&destination).expect("initialize the other");
    let err = handoff(&source, &destination).expect_err("another deployment refuses");
    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert_eq!(load_current(&destination), other, "untouched");
}

#[test]
fn handoff_refuses_a_same_deployment_destination_at_a_newer_generation() {
    let scratch = Scratch::new();
    let (source, destination) = (scratch.root.join("src.json"), scratch.root.join("dst.json"));
    assert_eq!(record(&source, "subject-a", &[]), 1);
    handoff(&source, &destination).expect("copy the source");
    // A handoff carries the salt sidecar with the journal, so the destination
    // resolves the salt the copied authority was recorded under and its own
    // denial can be recorded against it.
    std::fs::copy(salt_sidecar(&source), salt_sidecar(&destination))
        .expect("carry the deployment salt with the journal");
    assert_eq!(record(&destination, "subject-b", &[]), 2, "moved on");
    let kept = std::fs::read(&destination).expect("read back");
    let err = handoff(&source, &destination).expect_err("a newer destination refuses");
    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
    assert_eq!(std::fs::read(&destination).expect("read back"), kept);
}

#[test]
fn handoff_refuses_a_same_deployment_destination_with_different_denials() {
    let scratch = Scratch::new();
    let (source, destination) = (scratch.root.join("src.json"), scratch.root.join("dst.json"));
    assert_eq!(record(&source, "subject-a", &[]), 1);
    let body = std::fs::read_to_string(&source).expect("read the source");
    let forged = body.replacen("subject-a", "subject-b", 1);
    write_raw(&destination, forged.as_bytes());
    let kept = std::fs::read(&destination).expect("read back");
    let err = handoff(&source, &destination).expect_err("different denials refuse");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert_eq!(std::fs::read(&destination).expect("read back"), kept);
}

#[test]
fn handoff_refuses_a_damaged_destination() {
    let scratch = Scratch::new();
    let (source, destination) = (scratch.root.join("src.json"), scratch.root.join("dst.json"));
    initialize_new_empty(&source).expect("initialize the source");
    let damaged = b"{";
    write_raw(&destination, damaged);
    let err = handoff(&source, &destination).expect_err("a damaged destination refuses");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert_eq!(std::fs::read(&destination).expect("read back"), damaged);
}
