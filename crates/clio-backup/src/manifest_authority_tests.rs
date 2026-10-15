// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the manifest authority: the
//! forward-compatible `authority` field and its `salt_fingerprint`.
//!
//! ## Owns
//! - The authority-absent and fingerprint-absent parsing cases
//!
//! ## Does not own
//! - The manifest field set and the other refusal rules (`manifest_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the authority field or its fingerprint changes.
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the manifest authority.

use super::tests::{parsed_value, sample};
use super::*;

/// An older manifest with no `authority` key parses with `None`, and a `None`
/// authority writes no `authority` key back.
#[test]
fn an_older_manifest_without_authority_parses_as_none() {
    let mut value = parsed_value(&sample().to_json_bytes());
    value.as_object_mut().expect("object").remove("authority");
    let bytes = serde_json::to_vec(&value).expect("json");
    let parsed = BackupManifest::from_json_bytes(&bytes).expect("an older manifest parses");
    assert_eq!(parsed.authority, None);
    let json = parsed_value(&parsed.to_json_bytes());
    assert!(!json.as_object().expect("object").contains_key("authority"));
}

/// An authority written before the fingerprint field existed parses with
/// `None`, and a `None` fingerprint writes no `salt_fingerprint` key back, so
/// the field is additive on the wire.
#[test]
fn an_authority_without_a_salt_fingerprint_parses_as_none() {
    let mut value = parsed_value(&sample().to_json_bytes());
    value["authority"]
        .as_object_mut()
        .expect("authority object")
        .remove("salt_fingerprint");
    let bytes = serde_json::to_vec(&value).expect("json");
    let parsed = BackupManifest::from_json_bytes(&bytes).expect("an older authority parses");
    assert_eq!(
        parsed
            .authority
            .as_ref()
            .expect("authority")
            .salt_fingerprint,
        None
    );
    let json = parsed_value(&parsed.to_json_bytes());
    assert!(
        !json["authority"]
            .as_object()
            .expect("authority object")
            .contains_key("salt_fingerprint"),
        "a None fingerprint must not write the key: {json}"
    );
}
