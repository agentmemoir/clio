// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the backup manifest type:
//! determinism, round-trips, forward compatibility, and every refusal rule.
//!
//! ## Owns
//! - The manifest test fixtures and the T05/T06/T07/T13 manifest cases
//! - The key member's recorded size: read back, required with a key member,
//!   refused at zero, and round-tripped
//!
//! ## Does not own
//! - Container or inventory tests (see the sibling `*_tests.rs` files)
//!
//! ## Boundary
//!
//! Keep this file focused on `manifest::BackupManifest` behavior.

use super::*;
use clio_types::{AmError, ErrorCode};

pub(super) fn sample() -> BackupManifest {
    BackupManifest {
        kind: KIND.into(),
        format_version: FORMAT_VERSION,
        created_at: "2026-09-27T00:00:00Z".into(),
        tool_version: "clio 0.1.0".into(),
        store: StoreInfo {
            backend: "postgres".into(),
            slug: "memoir".into(),
            database: "clio".into(),
            schema: Some("public".into()),
        },
        snapshot: SnapshotInfo {
            method: "offline-copy".into(),
            consistency: "point-in-time".into(),
            started_at: "2026-09-27T00:00:00Z".into(),
            duration_ms: 42,
        },
        payload: PayloadInfo {
            member: "store.db".into(),
            bytes: 1024,
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            uncompressed_bytes: 2048,
        },
        counts: CountsInfo { complete: false },
        schema_version: Some("1".into()),
        keys: KeysInfo {
            included: true,
            member: Some(crate::KEY_MEMBER.into()),
            subjects: 3,
            bytes: Some(64),
            note: "key store copied; per-subject keys are wrapped and the Clio key is not included"
                .into(),
        },
        label: Some("nightly".into()),
        note: None,
        compression: COMPRESSION.into(),
        authority: Some(ManifestAuthority {
            deployment_id: "deploy-1".into(),
            generation: 7,
            salt_fingerprint: Some("fp-1".into()),
        }),
    }
}

fn corrupt(err: AmError) -> String {
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    err.message
}

/// The manifest round-trips: JSON bytes parse back into an equal struct.
#[test]
fn t10_json_round_trip() {
    let parsed = BackupManifest::from_json_bytes(&sample().to_json_bytes())
        .expect("a valid manifest parses");
    assert_eq!(parsed, sample());
}

/// T05: an unknown extra field in the JSON is ignored on read, so the
/// manifest field set can grow without a version bump. The nested case covers
/// the same rule inside a struct that has gained a field, which is what lets an
/// older build read a newer archive.
#[test]
fn t05_unknown_manifest_field_ignored() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&sample().to_json_bytes()).expect("valid JSON");
    value["future_field"] = serde_json::json!({"anything": [1, 2, 3]});
    value["keys"]["future_key_field"] = serde_json::json!("dropped by an older reader");
    let bytes = serde_json::to_vec(&value).expect("valid JSON");
    let parsed =
        BackupManifest::from_json_bytes(&bytes).expect("unknown fields are ignored on read");
    assert_eq!(parsed, sample());
}

/// T06: a wrong format_version, kind, or compression is refused with the
/// found and the supported value both named in the message.
#[test]
fn t06_wrong_kind_version_compression_refused() {
    let mut m = sample();
    m.format_version = 99;
    let msg = corrupt(
        BackupManifest::from_json_bytes(&m.to_json_bytes())
            .expect_err("format_version 99 must be refused"),
    );
    assert!(msg.contains("99"), "{msg}");
    assert!(msg.contains("supports 1"), "{msg}");

    let mut m = sample();
    m.kind = "something-else".into();
    let msg = corrupt(
        BackupManifest::from_json_bytes(&m.to_json_bytes())
            .expect_err("a foreign kind must be refused"),
    );
    assert!(msg.contains("something-else"), "{msg}");
    assert!(msg.contains(KIND), "{msg}");

    let mut m = sample();
    m.compression = "zstd".into();
    let msg = corrupt(
        BackupManifest::from_json_bytes(&m.to_json_bytes())
            .expect_err("a foreign compression must be refused"),
    );
    assert!(msg.contains("zstd") && msg.contains(COMPRESSION), "{msg}");

    let mut m = sample();
    m.store.backend = "oracle".into();
    let msg = corrupt(
        BackupManifest::from_json_bytes(&m.to_json_bytes())
            .expect_err("an unknown backend must be refused"),
    );
    assert!(msg.contains("oracle") && msg.contains("postgres"), "{msg}");
}

/// T07: the same manifest serializes byte-identically twice, and a
/// manifest built from the same inputs twice is byte-identical.
#[test]
fn t07_manifest_bytes_are_deterministic() {
    assert_eq!(sample().to_json_bytes(), sample().to_json_bytes());
}

/// Empty required strings are refused, naming the field.
#[test]
fn t06b_empty_required_strings_refused() {
    for field in [
        "created_at",
        "tool_version",
        "store.backend",
        "store.slug",
        "payload.member",
        "payload.sha256",
    ] {
        let mut m = sample();
        match field {
            "created_at" => m.created_at = String::new(),
            "tool_version" => m.tool_version = String::new(),
            "store.backend" => m.store.backend = String::new(),
            "store.slug" => m.store.slug = String::new(),
            "payload.member" => m.payload.member = String::new(),
            "payload.sha256" => m.payload.sha256 = String::new(),
            _ => unreachable!("table drift"),
        }
        let msg = corrupt(
            BackupManifest::from_json_bytes(&m.to_json_bytes())
                .expect_err("empty required strings are refused"),
        );
        assert!(msg.contains(field), "case {field}: {msg}");
    }
}

/// A garbage field shape fails cleanly with the serde error text, never a
/// panic; a wrong-length digest is refused by validation.
#[test]
fn t09_garbage_shape_fails_cleanly() {
    for mutation in [
        r#"{"payload": {"bytes": "not-a-number"}}"#,
        r#"{"snapshot": {"duration_ms": -5}}"#,
        r#"{"format_version": "1"}"#,
        r#"{"counts": {"complete": "yes"}}"#,
    ] {
        let bytes = splice_json_field(&sample().to_json_bytes(), mutation);
        let err = BackupManifest::from_json_bytes(&bytes).expect_err(mutation);
        assert_eq!(
            err.code,
            ErrorCode::ArchiveCorrupt,
            "{mutation}: {}",
            err.message
        );
    }

    let mut m = sample();
    m.payload.sha256 = "abc".into();
    let msg = corrupt(
        BackupManifest::from_json_bytes(&m.to_json_bytes())
            .expect_err("a wrong-length digest must be refused"),
    );
    assert!(msg.contains("payload.sha256"), "{msg}");
}

/// Replace the value of the top-level field named in `mutation` with the
/// JSON snippet's value.
fn splice_json_field(base: &[u8], mutation: &str) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(base).expect("valid JSON");
    let patch: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(mutation).expect("valid mutation JSON");
    for (key, val) in patch {
        value[key.as_str()] = val;
    }
    serde_json::to_vec(&value).expect("valid JSON")
}

/// T13: a `keys.included = false` manifest parses and serializes with the
/// flag verbatim; the reader never upgrades it to true.
#[test]
fn t13_keys_not_included_stays_verbatim() {
    let mut m = sample();
    m.keys = KeysInfo {
        included: false,
        member: None,
        subjects: 0,
        bytes: None,
        note: "the key store was not included".into(),
    };
    let parsed = BackupManifest::from_json_bytes(&m.to_json_bytes())
        .expect("keys.included = false is legal");
    assert!(!parsed.keys.included);
    assert_eq!(parsed.keys.member, None);
    assert_eq!(parsed.keys.note, "the key store was not included");
}

/// The manifest document with `keys.bytes` set, or with it removed when the
/// argument is `None`.
fn json_with_keys_bytes(bytes: Option<u64>) -> Vec<u8> {
    let mut value: serde_json::Value =
        serde_json::from_slice(&sample().to_json_bytes()).expect("valid JSON");
    let keys = value["keys"]
        .as_object_mut()
        .expect("the sample's keys is a JSON object");
    match bytes {
        Some(size) => keys.insert("bytes".into(), serde_json::json!(size)),
        None => keys.remove("bytes"),
    };
    serde_json::to_vec(&value).expect("valid JSON")
}

/// Forward compatibility, new-reader half: a manifest JSON carrying
/// `keys.bytes` parses, and the parsed struct holds the value from the
/// document rather than the sample's.
///
/// An older build reading this same archive has no `bytes` field to fill in,
/// and it must still parse: no manifest struct sets `deny_unknown_fields`, so
/// the field it has never heard of is dropped. The paired rule lives in
/// `an_included_key_manifest_without_a_recorded_size_is_refused`: this build
/// reads the field when it is present, and refuses an included-key archive
/// that never recorded a size.
#[test]
fn keys_bytes_is_read_from_the_json_document() {
    let bytes = json_with_keys_bytes(Some(1234));
    let parsed =
        BackupManifest::from_json_bytes(&bytes).expect("a manifest carrying the field parses");
    assert_eq!(
        parsed.keys.bytes,
        Some(1234),
        "the key size must come from the document, not from a default"
    );
    assert_ne!(
        parsed.keys.bytes,
        sample().keys.bytes,
        "the fixture carries a different size, so a dropped field would show up here"
    );
}

/// An archive that carries the key member must record its size. The document
/// with `keys.bytes` removed is the shape an archive written before the field
/// existed carried, and this build refuses it with the archive-corrupt error
/// instead of reading the size as unknown.
#[test]
fn an_included_key_manifest_without_a_recorded_size_is_refused() {
    let bytes = json_with_keys_bytes(None);
    assert!(
        !parsed_value(&bytes)["keys"]
            .as_object()
            .expect("keys is an object")
            .contains_key("bytes"),
        "the fixture must not carry the field at all"
    );
    let msg = corrupt(
        BackupManifest::from_json_bytes(&bytes)
            .expect_err("an archive with a key member and no recorded size is refused"),
    );
    assert!(msg.contains("keys.bytes"), "{msg}");
}

/// A recorded key size of zero is refused when the archive carries the key
/// member: a restore reserves room against this number, and nothing can be
/// staged into zero bytes.
#[test]
fn an_included_key_manifest_with_a_zero_size_is_refused() {
    let bytes = json_with_keys_bytes(Some(0));
    let msg = corrupt(
        BackupManifest::from_json_bytes(&bytes)
            .expect_err("a zero key size with a key member is refused"),
    );
    assert!(msg.contains("keys.bytes"), "{msg}");
}

/// A keyless archive serializes with no `bytes` key at all, so its bytes are
/// unchanged from before the field existed.
#[test]
fn a_keyless_archive_omits_the_key_size_from_its_json() {
    let mut m = sample();
    m.keys = KeysInfo {
        included: false,
        member: None,
        subjects: 0,
        bytes: None,
        note: "no key store file exists".into(),
    };
    let value = parsed_value(&m.to_json_bytes());
    let keys = value["keys"].as_object().expect("keys is a JSON object");
    assert!(
        !keys.contains_key("bytes"),
        "found keys = {keys:?}; a keyless archive must gain no bytes key"
    );
}

/// A keyless archive may still record a key size: the rule binds the size to
/// the key member's presence, and no existing rule constrains the value when
/// no member is carried. Zero included, so the rule stays purely conditional.
#[test]
fn a_keyless_archive_may_still_record_a_key_size() {
    let mut m = sample();
    m.keys = KeysInfo {
        included: false,
        member: None,
        subjects: 0,
        bytes: Some(0),
        note: "no key store file exists".into(),
    };
    let parsed = BackupManifest::from_json_bytes(&m.to_json_bytes())
        .expect("a keyless archive with a recorded size is legal");
    assert_eq!(parsed.keys.bytes, Some(0));
}

/// A recorded key size survives serialization and parsing unchanged.
#[test]
fn to_json_bytes_and_from_json_bytes_preserve_the_key_size() {
    let mut m = sample();
    m.keys.bytes = Some(4096);
    let parsed = BackupManifest::from_json_bytes(&m.to_json_bytes())
        .expect("a manifest carrying a key size parses");
    assert_eq!(
        parsed.keys.bytes,
        Some(4096),
        "the key size must round-trip"
    );
}

/// A manifest that declares the key included but names no member is refused:
/// the declaration flag and the member name must agree, and one saying the
/// archive carries the key member while the other says it does not is a
/// manifest no later check could read one way.
#[test]
fn an_included_key_without_a_member_name_is_refused() {
    let mut m = sample();
    m.keys.included = true;
    m.keys.member = None;
    m.keys.bytes = None;
    let msg = corrupt(
        BackupManifest::from_json_bytes(&m.to_json_bytes())
            .expect_err("an included key with no member name is refused"),
    );
    assert!(msg.contains("keys.included"), "{msg}");
    assert!(msg.contains("keys.member"), "{msg}");
}

/// A manifest that names a key member but declares it not included is
/// refused, the mirror of the case above: the two fields must agree in both
/// directions, or an included flag could smuggle a size rule past validation.
#[test]
fn a_named_key_member_declared_not_included_is_refused() {
    let mut m = sample();
    m.keys.included = false;
    let msg = corrupt(
        BackupManifest::from_json_bytes(&m.to_json_bytes())
            .expect_err("a named key member declared not included is refused"),
    );
    assert!(msg.contains("keys.included"), "{msg}");
    assert!(msg.contains("keys.member"), "{msg}");
}

/// Every combination of the key declaration shape parses or is refused. Only
/// the two agreeing shapes parse: no key member with no name, or a named key
/// member with a real size. A keyless archive may still record a size, as
/// `a_keyless_archive_may_still_record_a_key_size` pins.
#[test]
fn keys_declaration_shapes_parse_only_when_they_agree() {
    // (included, member named, bytes, parses).
    let cases = [
        (false, false, None, true),
        (false, false, Some(0), true),
        (false, false, Some(7), true),
        (false, true, None, false),
        (false, true, Some(0), false),
        (false, true, Some(7), false),
        (true, false, None, false),
        (true, false, Some(0), false),
        (true, false, Some(7), false),
        (true, true, None, false),
        (true, true, Some(0), false),
        (true, true, Some(7), true),
    ];
    for (included, named, bytes, parses) in cases {
        let mut m = sample();
        m.keys.included = included;
        m.keys.member = named.then(|| crate::KEY_MEMBER.into());
        m.keys.bytes = bytes;
        let result = BackupManifest::from_json_bytes(&m.to_json_bytes());
        assert_eq!(
            result.is_ok(),
            parses,
            "included={included} member_named={named} bytes={bytes:?}"
        );
    }
}

/// The manifest document as a JSON value, for assertions about its shape.
pub(super) fn parsed_value(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).expect("the manifest is valid JSON")
}
