// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the duplicate-member-name hostile cases:
//! archives that carry two members under one name, and the writer's own
//! once-only rule, asserted through the reader, the verify pass, the listing,
//! extraction, and the writer.
//!
//! ## Owns
//! - The hand-built duplicate-member fixtures (`duplicate_payload`,
//!   `duplicate_key`) and the assertions that a repeated name is refused on
//!   the second header, before the second copy is inflated
//! - The writer-side once-only rule's tests
//!
//! ## Does not own
//! - The reader's repeated-name rule itself (see `container_reader`)
//! - The other hostile-manifest cases (see `hostile_manifest_tests`)
//!
//! ## Boundary
//!
//! Changes to the repeated-name refusals' wording or to when they fire should
//! normally require changes here. Changes to the other hostile cases should
//! not.
//!
//! Keep this module focused on the duplicate-member-name rule.

use crate::KEY_MEMBER;
use crate::MANIFEST_MEMBER;
use crate::container::ArchiveWriter;
use crate::extract::extract_members;
use crate::fixtures::{Scratch, dir_entries};
use crate::inventory::{list_snapshot_rows, read_manifest};
use crate::tar_header::HEADER_SIZE;
use crate::verify::verify_archive;
use crate::verify::verify_hostile_manifest_tests::{
    KEYS, PAYLOAD, hostile_archive, integrity_ok, manifest_bytes, raw_member, refuse,
    verify_refuses,
};
use clio_types::ErrorCode::{ArchiveCorrupt, InvalidArgument};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A second, different key store for the duplicate-key case.
const OTHER_KEYS: &[u8] = b"SQLite format 3\0other-key-store";

/// The shared manifest fixture with the key size the manifest rule requires,
/// matching the first key member the archive carries. The shared fixture
/// records no size, so the value is injected here; the shared file is not
/// this module's to change.
fn keyed_manifest_bytes() -> Vec<u8> {
    let mut value: Value =
        serde_json::from_slice(&manifest_bytes(PAYLOAD, true, None)).expect("fixture JSON");
    value["keys"]["bytes"] = serde_json::json!(KEYS.len() as u64);
    serde_json::to_vec(&value).expect("fixture JSON")
}

/// Two members with the payload's name, built by hand. The reader refuses the
/// second name on its header, before a byte of the second copy is inflated,
/// and the refusal names the member. That only the first copy was inflated is
/// what the integrity-call count proves, and that the second copy may differ
/// is why the rule fires before either copy is chosen between.
#[test]
fn a_duplicated_payload_member_is_refused_before_the_second_copy_is_inflated() {
    let scratch = Scratch::new("hostile-dup-payload");
    let tampered: &[u8] = b"SQLite format 3\0tampered-copy";
    let mixed = duplicate_payload(&scratch, "dup-mixed", PAYLOAD, tampered);
    let mut calls = 0;

    let message = {
        let mut integrity = |_: &Path| {
            calls += 1;
            Ok(())
        };
        refuse(verify_archive(&mixed, &mut integrity), ArchiveCorrupt)
    };
    assert!(
        message.contains("repeats the member name"),
        "the rule: {message}"
    );
    assert!(message.contains("\"store.db\""), "the member: {message}");
    assert_eq!(calls, 1, "only the first copy was inflated: {message}");

    let rows = list_snapshot_rows(scratch.path(), None).expect("the listing is total");
    assert_eq!(rows.len(), 1, "the hostile file still lists");
    assert!(
        rows.iter().all(|row| row.unreadable.is_none()),
        "the manifests themselves still read: {:?}",
        rows.iter()
            .map(|row| row.unreadable.as_deref())
            .collect::<Vec<_>>()
    );
}

/// Two members with the payload's name. The writer refuses the second one, and
/// an archive built by hand with the repeat is refused by the reader, so the
/// second copy is never checked and never chosen between.
#[test]
fn a_duplicated_payload_member_is_refused_rather_than_read_twice() {
    let scratch = Scratch::new("hostile-dup-payload");
    let tampered: &[u8] = b"SQLite format 3\0tampered-copy";
    let same = duplicate_payload(&scratch, "dup-same", PAYLOAD, PAYLOAD);
    let mixed = duplicate_payload(&scratch, "dup-mixed", PAYLOAD, tampered);
    let calls = std::cell::Cell::new(0u32);
    let mut integrity = |_: &Path| {
        calls.set(calls.get() + 1);
        Ok(())
    };
    for (path, label) in [(&same, "identical"), (&mixed, "differing")] {
        calls.set(0);
        let message = refuse(verify_archive(path, &mut integrity), ArchiveCorrupt);
        assert!(
            message.contains("repeats the member name"),
            "{label} copies: {message}"
        );
        assert!(message.contains("store.db"), "{label} copies: {message}");
        assert_eq!(
            calls.get(),
            1,
            "{label} copies: the first payload is checked, the second never is"
        );
    }
}

/// Two members named `keys.kms.db`, one of them a different key store. The
/// manifest records no digest for the key member, so nothing else would catch
/// the swap; the repeated-name rule refuses the archive instead. Extraction
/// refuses with the same words and leaves no staged file behind.
#[test]
fn a_duplicated_key_member_is_refused_and_leaves_nothing_staged() {
    let scratch = Scratch::new("hostile-dup-keys");
    let path = duplicate_key(&scratch, "dup-keys");

    let message = verify_refuses(&path, ArchiveCorrupt);
    assert!(
        message.contains("repeats the member name"),
        "the rule: {message}"
    );
    assert!(message.contains(KEY_MEMBER), "the member: {message}");

    let staging = scratch.join("staged");
    std::fs::create_dir_all(&staging).expect("staging dir");
    let manifest = read_manifest(&path).expect("the archive's manifest");
    let error =
        extract_members(&path, &manifest, &staging).expect_err("the second key member is refused");
    assert_eq!(error.code, ArchiveCorrupt, "{}", error.message);
    assert_eq!(error.message, message, "extract and verify agree");
    assert!(
        dir_entries(&staging).is_empty(),
        "the first copy removed itself: {:?}",
        dir_entries(&staging)
    );
}

/// Two members named `keys.kms.db`, which is the case that mattered most: the
/// manifest records no digest for the key member, so two different key stores
/// would both pass and the consumer would pick one by member order alone. The
/// repeat is refused instead.
#[test]
fn a_duplicated_key_member_is_refused_before_either_key_store_is_read() {
    let scratch = Scratch::new("hostile-dup-keys");
    let path = duplicate_key(&scratch, "dup-keys");

    let message = refuse(verify_archive(&path, &mut integrity_ok), ArchiveCorrupt);

    assert!(message.contains("repeats the member name"), "{message}");
    assert!(message.contains(KEY_MEMBER), "{message}");
}

/// The writer refuses a second manifest member, and a hand-built archive with
/// the repeat is refused by the reader, so the manifest is never reported twice
/// and the two copies are never left uncompared.
#[test]
fn a_duplicated_manifest_member_is_refused_by_the_writer_and_the_reader() {
    let manifest = manifest_bytes(PAYLOAD, false, None);
    let mut writer = ArchiveWriter::new(Vec::new());
    writer.write_manifest(&manifest).expect("manifest first");
    let message = refuse(writer.write_manifest(&manifest), InvalidArgument);
    assert!(message.contains("already been written"), "{message}");
    let name = writer.add_member(MANIFEST_MEMBER, &b"x"[..]);
    assert!(refuse(name, InvalidArgument).contains("reserved"));

    let scratch = Scratch::new("hostile-dup-manifest");
    let mut raw = raw_member(MANIFEST_MEMBER, &manifest);
    raw.extend(raw_member(MANIFEST_MEMBER, &manifest));
    raw.extend(raw_member("store.db", PAYLOAD));
    raw.extend(vec![0u8; 2 * HEADER_SIZE]);
    let path = hostile_archive(scratch.path(), "dup-manifest", &raw);

    let message = refuse(verify_archive(&path, &mut integrity_ok), ArchiveCorrupt);
    assert!(message.contains("repeats the member name"), "{message}");
    assert!(message.contains(MANIFEST_MEMBER), "{message}");
}

/// The writer's own duplicate rule, for a payload name rather than the
/// manifest's: a second member under one name is refused at write time, so an
/// archive this build produces is always one this build can read back.
#[test]
fn the_writer_refuses_a_second_member_under_one_name() {
    let mut writer = ArchiveWriter::new(Vec::new());
    writer
        .write_manifest(&manifest_bytes(PAYLOAD, false, None))
        .expect("manifest first");
    writer.add_member("store.db", PAYLOAD).expect("first copy");

    let message = refuse(writer.add_member("store.db", PAYLOAD), InvalidArgument);
    assert!(
        message.contains("already carries a member named"),
        "{message}"
    );
    assert!(message.contains("a name appears once"), "{message}");

    let measured = writer.add_measured_member("store.db", &b"x"[..], 1);
    assert!(
        refuse(measured, InvalidArgument).contains("already carries a member named"),
        "the measured path carries the same rule"
    );
}

/// An archive with two `store.db` members, built by hand because the writer
/// refuses the repeat.
fn duplicate_payload(scratch: &Scratch, tag: &str, first: &[u8], second: &[u8]) -> PathBuf {
    let mut raw = raw_member(MANIFEST_MEMBER, &manifest_bytes(first, false, None));
    raw.extend(raw_member("store.db", first));
    raw.extend(raw_member("store.db", second));
    raw.extend(vec![0u8; 2 * HEADER_SIZE]);
    hostile_archive(scratch.path(), tag, &raw)
}

/// An archive that declares a key member and carries two different ones, built
/// by hand because the writer refuses the repeat.
fn duplicate_key(scratch: &Scratch, tag: &str) -> PathBuf {
    let mut raw = raw_member(MANIFEST_MEMBER, &keyed_manifest_bytes());
    raw.extend(raw_member("store.db", PAYLOAD));
    raw.extend(raw_member(KEY_MEMBER, KEYS));
    raw.extend(raw_member(KEY_MEMBER, OTHER_KEYS));
    raw.extend(vec![0u8; 2 * HEADER_SIZE]);
    hostile_archive(scratch.path(), tag, &raw)
}
