// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the one number the create writes for
//! the key member beyond what it already reported: the size recorded in the
//! manifest, which is what a restore's space pre-flight reserves against.
//!
//! ## Owns
//! - The claim that a created archive's manifest carries the key member's
//!   measured size, that a keyless archive carries none, and that a key store
//!   with no bytes is refused rather than recorded as zero
//!
//! ## Does not own
//! - The manifest's serde shape and its forward compatibility (see
//!   `manifest_tests`)
//! - The rest of archive creation (see `create_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{KeySource, create_archive};
use crate::KEY_MEMBER;
use crate::container::ArchiveReader;
use crate::fixtures::{
    Scratch, dir_entries, key_source, names_with_extension, request, staged_payload,
};
use crate::inventory::read_manifest;
use std::path::Path;

/// A created archive's manifest records the key member's measured size, so the
/// restore can reserve room for the key before it stages anything.
///
/// The number is the size the copy measured while it wrote the staged key
/// store, which is the same size the member is written into the archive under.
/// Asserting it against the key store file on disk is what ties the manifest's
/// number to the bytes the restore will actually write.
#[test]
fn a_created_archives_manifest_records_the_key_members_size() {
    let scratch = Scratch::new("create-key-size");
    let payload = staged_payload(scratch.path(), 21);
    let mut req = request(scratch.path(), payload);
    let key = key_source(scratch.path(), 1);
    let key_path = key.path.clone();
    req.keys = Some(key);

    let report = create_archive(&req).expect("create an archive carrying its key store");
    let expected = std::fs::metadata(&key_path)
        .expect("the key store file")
        .len();
    let manifest = read_manifest(&report.path).expect("read the manifest back");

    assert_eq!(
        manifest.keys.bytes,
        Some(expected),
        "the manifest's key size must be the key store's own size, so the restore reserves the \
         right number of bytes"
    );
    assert!(
        manifest.keys.included,
        "an archive that records a key size must also declare the key member"
    );
}

/// A store with no key store file records no key size at all, rather than a
/// zero that a reader could not tell from a measured empty key store.
#[test]
fn a_keyless_archives_manifest_records_no_key_size() {
    let scratch = Scratch::new("create-keyless-size");
    let payload = staged_payload(scratch.path(), 22);
    let req = request(scratch.path(), payload);
    assert!(
        req.keys.is_none(),
        "the fixture must build a keyless request"
    );

    let report = create_archive(&req).expect("create a keyless archive");
    let manifest = read_manifest(&report.path).expect("read the manifest back");

    assert_eq!(
        manifest.keys.bytes, None,
        "an archive with no key member must record no key size"
    );
    assert!(
        !manifest.keys.included,
        "the fixture must build a keyless archive"
    );
}

/// The size the archive's key member was written under, read back through the
/// container rather than assumed from the staged copy on disk.
fn key_member_size(path: &Path) -> Option<u64> {
    let file = std::fs::File::open(path).expect("open the archive");
    let mut reader = ArchiveReader::new(file).expect("read the archive");
    while let Some(header) = reader.next_member().expect("walk the archive") {
        if header.name == KEY_MEMBER {
            return Some(header.size);
        }
        reader.skip_current().expect("skip the member");
    }
    None
}

/// The recorded key size is not just present: it is the size the key member
/// was actually written under inside the archive, so the restore's space
/// reservation matches the bytes it will read.
///
/// Reading the manifest back also runs the manifest's own validation, so the
/// round trip only passes when the keyed shape is valid: a key member named
/// with a real, non-zero size.
#[test]
fn the_recorded_key_size_agrees_with_the_key_member_written_into_the_archive() {
    let scratch = Scratch::new("create-key-member-size");
    let payload = staged_payload(scratch.path(), 23);
    let mut req = request(scratch.path(), payload);
    req.keys = Some(key_source(scratch.path(), 2));

    let report = create_archive(&req).expect("create an archive carrying its key store");
    let manifest = read_manifest(&report.path).expect("read the manifest back");
    let written = key_member_size(&report.path).expect("the archive carries a key member");

    assert_eq!(manifest.keys.member.as_deref(), Some(KEY_MEMBER));
    assert_eq!(
        manifest.keys.bytes,
        Some(written),
        "the recorded size must be the member's own size, so the restore reserves the right bytes"
    );
    assert!(
        manifest.keys.bytes.is_some_and(|size| size > 0),
        "a carried key member records a real size"
    );
}

/// An empty key store has no size a manifest may record: the create refuses
/// before anything is written, says which field failed, and leaves no archive,
/// no temporary, and no staged key store behind.
#[test]
fn an_empty_key_store_is_refused_with_a_clear_error_and_no_leftovers() {
    let scratch = Scratch::new("create-empty-keys");
    let payload = staged_payload(scratch.path(), 24);
    let mut req = request(scratch.path(), payload);
    let empty = scratch.join("empty-keys.kms.json");
    std::fs::write(&empty, b"").expect("write the empty key store");
    req.keys = Some(KeySource {
        path: empty,
        subjects: 1,
    });

    let error = create_archive(&req).expect_err("an empty key store cannot be recorded");

    assert_eq!(error.code, clio_types::ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains("keys.bytes"),
        "the refusal names the field: {}",
        error.message
    );
    // The caller still owns the staged payload and the key source, so drop the
    // request the way the CLI does; then nothing from the refused run remains.
    drop(req);
    let entries = dir_entries(scratch.path());
    assert!(
        entries
            .iter()
            .all(|name| !name.contains("clio-snapshot-keys")),
        "a staged key store survived the refused run: {entries:?}"
    );
    assert!(
        names_with_extension(scratch.path(), "gz").is_empty(),
        "an archive appeared: {entries:?}"
    );
    assert!(
        names_with_extension(scratch.path(), "tmp").is_empty(),
        "a temporary survived: {entries:?}"
    );
}
