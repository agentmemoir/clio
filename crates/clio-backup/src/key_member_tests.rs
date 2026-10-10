// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the restore-side key-member size refusals:
//! a manifest that records no size for its key member, a zero size, or a size
//! the member's own header disagrees with is refused, and a refusal after
//! something was staged leaves the staging directory empty.
//!
//! ## Owns
//!
//! - The manifest-read refusals for a missing and a zero `keys.bytes`
//! - The declared-versus-header size refusal on the restore path, and the
//!   empty-staging assertion that goes with it
//! - The keyed-manifest fixture helper the cases share
//!
//! ## Does not own
//!
//! - The refusals themselves (see `key_member` and `manifest`)
//! - The keyless and magic-head extraction cases (see `extract_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on the key member's recorded size on the restore
//! path.

use crate::KEY_MEMBER;
use crate::MANIFEST_MEMBER;
use crate::extract::extract_members;
use crate::fixtures::{Scratch, dir_entries};
use crate::verify::verify_hostile_manifest_tests::{KEYS, PAYLOAD, hostile_archive, manifest_bytes, raw_member};
use crate::inventory::read_manifest;
use crate::tar_header::{HEADER_SIZE, build_header};
use clio_types::ErrorCode::ArchiveCorrupt;
use serde_json::Value;

/// The shared manifest fixture with the `keys.bytes` value the manifest rule
/// requires. The shared fixture records no size, so the value is injected
/// here; the shared file is not this module's to change.
fn keyed_manifest_bytes(size: u64) -> Vec<u8> {
    let mut value: Value =
        serde_json::from_slice(&manifest_bytes(PAYLOAD, true, None)).expect("fixture JSON");
    value["keys"]["bytes"] = serde_json::json!(size);
    serde_json::to_vec(&value).expect("fixture JSON")
}

/// One raw ustar member whose header declares `declared` bytes while the body
/// holds whatever bytes the caller supplies; the padding follows the body.
fn raw_member_declaring(name: &str, declared: u64, body: &[u8]) -> Vec<u8> {
    let mut out = build_header(name, declared).expect("header").to_vec();
    out.extend_from_slice(body);
    out.extend(crate::fixtures::pad(body));
    out
}

/// A manifest that declares the key included but names no member is refused
/// on read, so a restore never reaches extraction and nothing can be
/// promoted. The fixture below is the keyless shape with only the flag
/// flipped, which is exactly the inconsistent shape no earlier rule covered.
#[test]
fn a_manifest_that_declares_included_but_names_no_member_is_refused_on_read() {
    let scratch = Scratch::new("key-member-included-no-name");
    let mut value: Value =
        serde_json::from_slice(&manifest_bytes(PAYLOAD, false, None)).expect("fixture JSON");
    value["keys"]["included"] = serde_json::json!(true);
    let manifest = serde_json::to_vec(&value).expect("fixture JSON");
    let mut raw = raw_member(MANIFEST_MEMBER, &manifest);
    raw.extend(raw_member("store.db", PAYLOAD));
    raw.extend(raw_member(KEY_MEMBER, KEYS));
    raw.extend([0u8; 2 * HEADER_SIZE]);
    let archive = hostile_archive(scratch.path(), "included-no-member", &raw);

    let error = read_manifest(&archive).expect_err("the inconsistent shape is refused");

    assert_eq!(error.code, ArchiveCorrupt, "{}", error.message);
    assert!(
        error.message.contains("keys.included") && error.message.contains("keys.member"),
        "both fields are named: {}",
        error.message
    );
}

/// The old included-key archive shape, whose manifest records no key size at
/// all: the manifest read refuses it, so a restore never reaches extraction
/// and nothing can be promoted.
#[test]
fn a_manifest_that_records_no_key_size_is_refused_on_read() {
    let scratch = Scratch::new("key-member-no-size");
    // `manifest_bytes(PAYLOAD, true, None)` is exactly the old shape: a key
    // member declared, and no `keys.bytes` recorded for it.
    let mut raw = raw_member(MANIFEST_MEMBER, &manifest_bytes(PAYLOAD, true, None));
    raw.extend(raw_member("store.db", PAYLOAD));
    raw.extend(raw_member(KEY_MEMBER, KEYS));
    raw.extend([0u8; 2 * HEADER_SIZE]);
    let archive = hostile_archive(scratch.path(), "no-key-size", &raw);

    let error = read_manifest(&archive).expect_err("the old shape is refused");

    assert_eq!(error.code, ArchiveCorrupt, "{}", error.message);
    assert!(
        error.message.contains("keys.bytes"),
        "the missing field is named: {}",
        error.message
    );
}

/// A manifest that records a zero key size is refused on read: a size of zero
/// describes no key member a restore could use.
#[test]
fn a_manifest_that_records_a_zero_key_size_is_refused_on_read() {
    let scratch = Scratch::new("key-member-zero-size");
    let mut raw = raw_member(MANIFEST_MEMBER, &keyed_manifest_bytes(0));
    raw.extend(raw_member("store.db", PAYLOAD));
    raw.extend(raw_member(KEY_MEMBER, KEYS));
    raw.extend([0u8; 2 * HEADER_SIZE]);
    let archive = hostile_archive(scratch.path(), "zero-key-size", &raw);

    let error = read_manifest(&archive).expect_err("a zero size is refused");

    assert_eq!(error.code, ArchiveCorrupt, "{}", error.message);
    assert!(
        error.message.contains("greater than zero"),
        "the rule is named: {}",
        error.message
    );
}

/// The key member's header disagrees with the size the manifest records. The
/// member is refused before anything is staged for it, the payload staged
/// before it removes itself again, and the staging directory is left empty.
#[test]
fn a_key_member_whose_size_the_manifest_disagrees_with_is_refused_before_it_is_staged() {
    let scratch = Scratch::new("key-member-size-mismatch");
    let declared = KEYS.len() as u64;
    let mut raw = raw_member(MANIFEST_MEMBER, &keyed_manifest_bytes(declared));
    raw.extend(raw_member("store.db", PAYLOAD));
    // The header claims one more byte than the manifest records, so the
    // member cannot be the one the manifest describes.
    raw.extend(raw_member_declaring(KEY_MEMBER, declared + 1, KEYS));
    raw.extend([0u8; 2 * HEADER_SIZE]);
    let archive = hostile_archive(scratch.path(), "key-size-mismatch", &raw);

    let manifest = read_manifest(&archive).expect("the manifest is valid");
    let staging = scratch.join("staged");
    std::fs::create_dir_all(&staging).expect("staging dir");

    let error = extract_members(&archive, &manifest, &staging).expect_err("the member is refused");

    assert_eq!(error.code, ArchiveCorrupt, "{}", error.message);
    assert!(
        error.message.contains(KEY_MEMBER),
        "the key member is named: {}",
        error.message
    );
    assert!(
        error.message.contains(&declared.to_string())
            && error.message.contains(&(declared + 1).to_string()),
        "both sizes are named: {}",
        error.message
    );
    assert!(
        dir_entries(&staging).is_empty(),
        "nothing staged and nothing left behind: {:?}",
        dir_entries(&staging)
    );
}
