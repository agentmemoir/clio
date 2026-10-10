// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for what a restore stages beside the operator's
//! store: the staging-directory failures that come from the machine rather than
//! the archive, and the assertion the extraction suite shares about what a
//! staging directory is left holding.
//!
//! ## Owns
//! - The staging-directory refusal and the code it carries, for the payload
//!   member and for the key member
//! - The codes a staging failure must not lose: out of space stays out of space,
//!   and archive damage stays archive damage
//! - `staged_names`, the shared assertion that a staging directory holds exactly
//!   the staged members and nothing else
//!
//! ## Does not own
//! - The member digest and size refusals (see `extract_tests`)
//! - The repeated-member-name refusals (see `hostile_duplicate_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on the extraction paths a hostile archive is not
//! responsible for.

use crate::KEY_MEMBER;
use crate::MANIFEST_MEMBER;
use crate::extract::{extract_members, staging_failure};
use crate::fixtures::{Scratch, dir_entries};
use crate::tar_header::HEADER_SIZE;
use crate::verify::verify_hostile_manifest_tests::{
    KEYS, PAYLOAD, hostile_archive, manifest_bytes, raw_member,
};
use clio_types::{AmError, ErrorCode};
use serde_json::Value;
use std::path::Path;

/// Assert `dir` holds exactly the staged `files`, and return their names.
///
/// The run's own staging journal is a record of the staged paths rather than a
/// staged member, so it is the one name this comparison excludes; a staged
/// member nobody claimed would still fail the assertion.
///
/// It lives here, and not in `extract_tests`, because it is an assertion about
/// a staging directory rather than a member digest, and the file that owns the
/// staging cases is the one that had room to grow.
pub(crate) fn staged_names(dir: &Path, files: &[&Path]) -> Vec<String> {
    let mut names: Vec<String> = files
        .iter()
        .map(|path| {
            path.file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    // `dir_entries` is already sorted, so filtering leaves both sides ordered.
    let staged: Vec<String> = dir_entries(dir)
        .into_iter()
        .filter(|name| !name.ends_with(".journal"))
        .collect();
    assert_eq!(staged, names, "nothing else is staged in {dir:?}");
    names
}

/// A minimal well-formed archive: a manifest, a payload, and the end marker.
fn well_formed(scratch: &Scratch) -> std::path::PathBuf {
    let mut raw = raw_member(MANIFEST_MEMBER, &manifest_bytes(PAYLOAD, false, None));
    raw.extend(raw_member("store.db", PAYLOAD));
    raw.extend(vec![0u8; 2 * HEADER_SIZE]);
    hostile_archive(scratch.path(), "staging-fault", &raw)
}

#[test]
fn a_staging_directory_that_cannot_hold_the_member_is_not_reported_as_a_damaged_archive() {
    let scratch = Scratch::new("extract-unstaged");
    let archive = well_formed(&scratch);
    // A regular file where the staging directory belongs: the member cannot be
    // created, and nothing about the archive is wrong.
    let not_a_directory = scratch.join("not-a-directory");
    std::fs::write(&not_a_directory, b"x").expect("seed the blocking file");
    let manifest = crate::inventory::read_manifest(&archive).expect("the archive's manifest");

    let error = extract_members(&archive, &manifest, &not_a_directory)
        .expect_err("the member cannot be staged");

    // The filesystem's own code, so an out-of-space or unwritable-directory
    // condition is not reported to the operator as a corrupt archive.
    assert_ne!(
        error.code,
        ErrorCode::ArchiveCorrupt,
        "a staging failure is not archive corruption: {}",
        error.message
    );
    assert!(
        error.message.contains("could not be staged"),
        "the message says what happened: {}",
        error.message
    );
    assert!(
        error.message.contains("store.db"),
        "the message names the member: {}",
        error.message
    );
}

/// The shared manifest fixture with the key size the manifest rule requires,
/// matching the key member this archive actually carries. The shared fixture
/// records no size, so the value is injected here; the shared file is not
/// this module's to change.
fn keyed_manifest_bytes() -> Vec<u8> {
    let mut value: Value =
        serde_json::from_slice(&manifest_bytes(PAYLOAD, true, None)).expect("fixture JSON");
    value["keys"]["bytes"] = serde_json::json!(KEYS.len() as u64);
    serde_json::to_vec(&value).expect("fixture JSON")
}

/// A well-formed archive whose key member comes before the payload, so the key
/// member is the first one staged and the first staging failure belongs to it.
fn keys_first(scratch: &Scratch) -> std::path::PathBuf {
    let mut raw = raw_member(MANIFEST_MEMBER, &keyed_manifest_bytes());
    raw.extend(raw_member(KEY_MEMBER, KEYS));
    raw.extend(raw_member("store.db", PAYLOAD));
    raw.extend(vec![0u8; 2 * HEADER_SIZE]);
    hostile_archive(scratch.path(), "staging-key-fault", &raw)
}

/// The issue's own case: the staged key file cannot be created. The filesystem
/// gives its reason here (the directory is not a directory), and a full or
/// over-quota directory is the same shape with a different reason, so the point
/// is that the reason comes from the machine and the archive is never blamed for
/// it. The key member is staged first, so this is that member's own staging
/// failure and not the payload's.
#[test]
fn a_staging_failure_for_the_key_member_is_not_reported_as_a_damaged_archive() {
    let scratch = Scratch::new("extract-unstaged-key");
    let archive = keys_first(&scratch);
    let not_a_directory = scratch.join("not-a-directory");
    std::fs::write(&not_a_directory, b"x").expect("seed the blocking file");
    let manifest = crate::inventory::read_manifest(&archive).expect("the archive's manifest");

    let error = extract_members(&archive, &manifest, &not_a_directory)
        .expect_err("the key member cannot be staged");

    assert_ne!(
        error.code,
        ErrorCode::ArchiveCorrupt,
        "a staging failure is not archive corruption: {}",
        error.message
    );
    assert!(
        error.message.contains(KEY_MEMBER),
        "the message names the key member: {}",
        error.message
    );
    assert!(
        error.message.contains("could not be staged"),
        "the message says what happened: {}",
        error.message
    );
}

/// A staging failure that is the machine's condition keeps the code that
/// condition already carries. An operator told "damaged archive" goes looking for
/// damage that is not there, so a full volume has to arrive as out of space.
#[test]
fn a_staging_failure_keeps_the_out_of_space_code_it_arrives_with() {
    let error = staging_failure(
        KEY_MEMBER,
        "could not be staged",
        &AmError::new(
            ErrorCode::QuotaExceeded,
            "creating the staged member: No space left on device",
        ),
    );

    assert_eq!(error.code, ErrorCode::QuotaExceeded, "{}", error.message);
    assert!(
        error.message.contains(KEY_MEMBER),
        "the member is named: {}",
        error.message
    );
}

/// The other half of the same claim: real damage still arrives as
/// `ArchiveCorrupt`, so a helper that started re-labelling every failure could
/// not lose the out-of-space case without this one failing too.
#[test]
fn a_staging_failure_of_a_damaged_member_keeps_its_archive_corrupt_code() {
    let error = staging_failure(
        "store.db",
        "could not be staged",
        &AmError::new(
            ErrorCode::ArchiveCorrupt,
            "declared 100 bytes but the stream ended after 40",
        ),
    );

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{}", error.message);
}
