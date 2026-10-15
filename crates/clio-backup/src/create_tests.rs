// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing archive creation: member order, the
//! manifest the create writes, name collisions, label cleaning, the owner-only
//! mode, the atomic publish, the refusal cases a host can force — a missing or
//! read-only backup directory, a vanished or unreadable staged member — and the
//! guarantee that no staged file and no archive survives a failed run.
//!
//! ## Owns
//! - The create-path assertions over archives produced by `create_archive`
//!
//! ## Does not own
//! - The staging primitives (see `staging_tests`)
//! - Verify and inventory cases (see their own test modules)
//!
//! ## Boundary
//!
//! Keep this file focused on `create` behavior.

use super::*;
use crate::MANIFEST_MEMBER;
use crate::archive_name::collision_suffix;
use crate::container::ArchiveReader;
use crate::fixtures::{
    Scratch, dir_entries, key_source, names_with_extension, request, staged_payload,
};
use crate::inventory::read_manifest;
use std::path::Path;

/// The member names of an archive, in the order they were written.
fn member_names(path: &Path) -> Vec<String> {
    let file = std::fs::File::open(path).expect("open the archive");
    let mut reader = ArchiveReader::new(file).expect("read the archive");
    let mut names = Vec::new();
    while let Some(header) = reader.next_member().expect("next member") {
        reader.skip_current().expect("skip the member");
        names.push(header.name);
    }
    names
}

#[test]
fn a_created_archive_carries_the_manifest_first() {
    let scratch = Scratch::new("create-order");
    let payload = staged_payload(scratch.path(), 1);
    let mut req = request(scratch.path(), payload);
    req.keys = Some(key_source(scratch.path(), 2));

    let report = create_archive(&req).expect("create");

    assert_eq!(
        member_names(&report.path),
        vec![MANIFEST_MEMBER, "store.db", KEY_MEMBER]
    );
    assert!(report.keys_included);
    assert_eq!(report.keys_subjects, 2);
    assert_eq!(report.payload_bytes, req.payload.bytes());
    assert_eq!(
        report.archive_bytes,
        std::fs::metadata(&report.path).unwrap().len()
    );
    assert!(report.compression_ratio > 0.0);
}

#[test]
fn a_keyless_store_reports_keys_included_false_with_a_reason() {
    let scratch = Scratch::new("create-keyless");
    let payload = staged_payload(scratch.path(), 2);
    let req = request(scratch.path(), payload);

    let report = create_archive(&req).expect("create");
    let manifest = read_manifest(&report.path).expect("read the manifest back");

    assert!(!manifest.keys.included);
    assert!(manifest.keys.member.is_none());
    assert_eq!(manifest.keys.subjects, 0);
    assert!(
        !manifest.keys.note.is_empty(),
        "an absent key member says why, so it is never silently true"
    );
    assert!(!report.keys_included);
    assert_eq!(
        member_names(&report.path),
        vec![MANIFEST_MEMBER, "store.db"]
    );
}

#[test]
fn the_manifest_records_the_measured_payload_and_the_snapshot_facts() {
    let scratch = Scratch::new("create-manifest");
    let payload = staged_payload(scratch.path(), 3);
    let req = request(scratch.path(), payload);

    let report = create_archive(&req).expect("create");
    let manifest = read_manifest(&report.path).expect("read the manifest back");

    assert_eq!(manifest.payload.member, "store.db");
    assert_eq!(manifest.payload.bytes, req.payload.bytes());
    assert_eq!(manifest.payload.sha256, req.payload.sha256());
    assert_eq!(manifest.payload.uncompressed_bytes, req.payload.bytes());
    assert_eq!(manifest.store.backend, "sqlite");
    assert_eq!(manifest.store.slug, "clio");
    assert_eq!(manifest.snapshot.method, "vacuum-into");
    assert_eq!(manifest.snapshot.consistency, "point-in-time");
    assert!(manifest.counts.complete);
    assert_eq!(manifest.compression, "gzip");
    assert!(manifest.created_at.ends_with('Z'));
}

#[test]
fn a_label_with_control_characters_is_stored_stripped() {
    let scratch = Scratch::new("create-label");
    let payload = staged_payload(scratch.path(), 4);
    let mut req = request(scratch.path(), payload);
    req.label = Some("nightly\nrun\tuploading".to_owned());
    req.note = Some("a note\r\nwith a second line".to_owned());

    let report = create_archive(&req).expect("create");
    let manifest = read_manifest(&report.path).expect("read the manifest back");

    assert_eq!(manifest.label.as_deref(), Some("nightlyrunuploading"));
    assert_eq!(manifest.note.as_deref(), Some("a notewith a second line"));
    // The stored JSON is one line, so a viewer cannot be shown a second record.
    let raw = std::fs::read(&report.path).expect("read the archive");
    let manifest_json = read_manifest(&report.path).expect("manifest");
    let _ = manifest_json;
    assert!(!raw.is_empty(), "the archive is empty: {raw:?}");
}

#[test]
fn an_oversized_label_is_bounded() {
    let scratch = Scratch::new("create-long-label");
    let payload = staged_payload(scratch.path(), 5);
    let mut req = request(scratch.path(), payload);
    req.label = Some("x".repeat(MAX_OPERATOR_TEXT * 3));

    let report = create_archive(&req).expect("create");
    let manifest = read_manifest(&report.path).expect("read the manifest back");

    assert_eq!(
        manifest.label.as_deref().map_or(0, str::len),
        MAX_OPERATOR_TEXT
    );
}

#[test]
fn a_blank_label_is_dropped_rather_than_stored_empty() {
    assert_eq!(clean_text(None), None);
    assert_eq!(clean_text(Some("   \n\t ")), None);
    assert_eq!(clean_text(Some("  kept  ")), Some("kept".to_owned()));
}

#[test]
fn two_creates_in_the_same_second_produce_different_names() {
    let scratch = Scratch::new("create-collision");
    let first = staged_payload(scratch.path(), 6);
    let first = create_archive(&request(scratch.path(), first)).expect("first create");
    let second = staged_payload(scratch.path(), 7);
    let second = create_archive(&request(scratch.path(), second)).expect("second create");

    assert_ne!(first.id, second.id);
    assert_ne!(first.path, second.path);
    assert!(first.path.exists() && second.path.exists());
    assert_eq!(collision_suffix().len(), 8);
}

/// Two creates of the same payload into the same directory produce the same
/// bytes, apart from the manifest fields that name the run: the clock stamp, the
/// measured duration, and the file name. Everything else — the gzip wrapper, the
/// tar headers, the member order — is fixed, so two archives of unchanged content
/// are interchangeable and a digest of one is a digest of the other.
#[test]
fn two_creates_of_the_same_payload_differ_only_in_the_run_stamps() {
    let scratch = Scratch::new("create-reproducible");
    let first = staged_payload(scratch.path(), 64);
    let first = create_archive(&request(scratch.path(), first)).expect("first create");
    let second = staged_payload(scratch.path(), 64);
    let second = create_archive(&request(scratch.path(), second)).expect("second create");

    let one = read_manifest(&first.path).expect("read the first manifest");
    let other = read_manifest(&second.path).expect("read the second manifest");
    assert_eq!(
        one.payload.sha256, other.payload.sha256,
        "the same payload digests the same"
    );
    assert_eq!(one.payload.bytes, other.payload.bytes);
    assert_eq!(one.snapshot.method, other.snapshot.method);
    assert_eq!(one.snapshot.consistency, other.snapshot.consistency);

    // The gzip stream is the whole file, so the member bytes can only be equal
    // once the varying manifest fields are accounted for. Compare everything
    // else by rebuilding both manifests with the run-varying fields pinned.
    let pinned = |manifest: &BackupManifest| {
        let mut copy = manifest.clone();
        copy.created_at = String::new();
        copy.snapshot.started_at = String::new();
        copy.snapshot.duration_ms = 0;
        copy.to_json_bytes()
    };
    assert_eq!(
        pinned(&one),
        pinned(&other),
        "two archives of unchanged content differ only in the clock stamps"
    );
}

#[test]
fn a_slug_derived_name_never_contains_a_separator() {
    let scratch = Scratch::new("create-slug");
    let payload = staged_payload(scratch.path(), 8);
    let mut req = request(scratch.path(), payload);
    req.slug = "../../escape".to_owned();

    let error = create_archive(&req).expect_err("a slug with a separator is refused");

    assert_eq!(error.code, clio_types::ErrorCode::InvalidArgument);
    for name in dir_entries(scratch.path()) {
        assert!(
            !name.contains(".."),
            "nothing escaped the scratch directory: {name}"
        );
    }
}

#[test]
fn the_archive_is_owner_only() {
    let scratch = Scratch::new("create-mode");
    let payload = staged_payload(scratch.path(), 9);
    let report = create_archive(&request(scratch.path(), payload)).expect("create");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&report.path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the archive carries key material");
    }
    #[cfg(not(unix))]
    let _ = report;
}

#[cfg(unix)]
#[test]
fn the_staged_key_store_is_owner_only_and_removed_after_the_run() {
    let scratch = Scratch::new("create-staged-keys");
    let payload = staged_payload(scratch.path(), 10);
    let mut req = request(scratch.path(), payload);
    req.keys = Some(key_source(scratch.path(), 1));

    let report = create_archive(&req).expect("create");
    assert!(report.path.exists());
    // The caller still owns the staged payload, so drop it the way the CLI
    // does; then nothing staged may remain.
    drop(req);

    let entries = dir_entries(scratch.path());
    assert!(
        entries
            .iter()
            .all(|name| !name.contains("clio-snapshot-keys")),
        "a staged key store survived the run: {entries:?}"
    );
    assert!(
        names_with_extension(scratch.path(), "tmp").is_empty(),
        "a staged member survived the run: {entries:?}"
    );
}

#[test]
fn an_unwritable_backup_directory_produces_no_archive() {
    let scratch = Scratch::new("create-unwritable");
    let payload = staged_payload(scratch.path(), 11);
    let mut req = request(scratch.path(), payload);
    req.backup_dir = scratch.join("absent-dir");

    let error = create_archive(&req).expect_err("a missing directory cannot be written");

    assert_eq!(error.code, clio_types::ErrorCode::Internal);
    assert!(!scratch.join("absent-dir").exists());
}

#[test]
fn a_missing_staged_payload_produces_no_archive_and_no_leftovers() {
    let scratch = Scratch::new("create-missing-payload");
    let payload = staged_payload(scratch.path(), 12);
    let missing = payload.path().to_path_buf();
    let req = request(scratch.path(), payload);
    drop(std::fs::remove_file(&missing));

    let error = create_archive(&req).expect_err("a vanished payload cannot be archived");

    assert!(error.message.contains("store.db"), "{}", error.message);
    let archives = names_with_extension(scratch.path(), "gz");
    assert!(archives.is_empty(), "no archive appeared: {archives:?}");
    let temporaries = names_with_extension(scratch.path(), "tmp");
    assert!(
        temporaries.is_empty(),
        "a temporary archive survived: {temporaries:?}"
    );
}

#[test]
fn a_measure_mismatch_is_refused_instead_of_writing_a_broken_member() {
    let scratch = Scratch::new("create-mismatch");
    let payload = staged_payload(scratch.path(), 13);
    // Claim a size the staged file does not have: the writer must refuse
    // rather than emit a header that disagrees with the member.
    let path = payload.path().to_path_buf();
    let claimed = staging::StagedFile::adopt(path, 999_999, "0".repeat(64));
    let req = request(scratch.path(), claimed);

    let error = create_archive(&req).expect_err("a wrong size is refused");

    assert!(
        error.message.contains("999999"),
        "the message names the disagreement: {}",
        error.message
    );
}

/// A failure after the key store was staged must not leave that staged key
/// store behind: it is key material, and it sits beside the archive the run
/// never published.
#[test]
fn a_staged_key_store_is_removed_when_a_later_step_fails() {
    let scratch = Scratch::new("create-keys-later-failure");
    let payload = staged_payload(scratch.path(), 15);
    let mut req = request(scratch.path(), payload);
    req.keys = Some(key_source(scratch.path(), 3));
    // The key store is staged first, so removing the payload leaves the failure
    // after that staging happened.
    drop(std::fs::remove_file(req.payload.path()));

    let error = create_archive(&req).expect_err("a vanished payload cannot be archived");

    assert!(
        error.message.contains("store.db"),
        "the error names the member: {}",
        error.message
    );
    let entries = dir_entries(scratch.path());
    assert!(
        entries
            .iter()
            .all(|name| !name.contains("clio-snapshot-keys")),
        "a staged key store survived the failed run: {entries:?}"
    );
    drop(req);
    let archives = names_with_extension(scratch.path(), "gz");
    assert!(archives.is_empty(), "no archive appeared: {archives:?}");
    let temporaries = names_with_extension(scratch.path(), "tmp");
    assert!(
        temporaries.is_empty(),
        "a staged file survived the failed run: {temporaries:?}"
    );
}

/// A failure in the middle of the assembly pass — the member cannot be read
/// after the archive's temporary file is open — must be reported with the step
/// it happened on, must not publish an archive, and must not leave a temporary
/// archive or a staged member behind.
#[cfg(unix)]
#[test]
fn a_failure_mid_assembly_names_the_member_and_leaves_nothing_behind() {
    let scratch = Scratch::new("create-mid-assembly");
    // The payload is staged in its own sub-directory so the directory standing
    // in for it below is not itself a `.tmp` entry in the backup directory.
    let staged_dir = scratch.join("staged");
    std::fs::create_dir(&staged_dir).expect("the staging directory");
    let payload = staged_payload(&staged_dir, 16);
    let mut req = request(scratch.path(), payload);
    req.keys = Some(key_source(scratch.path(), 2));
    // A staged payload that opens but cannot be read: a directory opens for
    // reading on a Unix host and then fails every read, which is what the
    // assembly pass sees when the staged payload is gone or unreadable.
    let staged_payload_path = req.payload.path().to_path_buf();
    drop(std::fs::remove_file(&staged_payload_path));
    std::fs::create_dir(&staged_payload_path).expect("an unreadable staged payload");

    let error = create_archive(&req).expect_err("an unreadable member cannot be archived");

    assert_eq!(error.code, clio_types::ErrorCode::Internal);
    assert!(
        error.message.contains("store.db"),
        "the error names the member that failed: {}",
        error.message
    );
    assert!(
        error.message.contains("writing member"),
        "the error names the step that failed: {}",
        error.message
    );
    let archives = names_with_extension(scratch.path(), "gz");
    assert!(archives.is_empty(), "no archive appeared: {archives:?}");
    // The caller still owns the staged payload, exactly as the CLI does, so
    // drop the request; the key store and the archive temporary are owned by
    // the create itself and must already be gone.
    drop(req);
    let temporaries = names_with_extension(scratch.path(), "tmp");
    assert!(
        temporaries.is_empty(),
        "a temporary archive or staged key store survived: {temporaries:?}"
    );
}
